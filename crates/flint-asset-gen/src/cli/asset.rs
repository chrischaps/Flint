//! The ai package's subcommands of `flint asset` (#1553): the provider
//! calls, their jobs, the ledger and the style checks. Once flint-cli's
//! `asset.rs` under `#[cfg(feature = "ai")]`; now registered under the
//! kernel's `asset` by [`super::extensions`], so a CLI built without the
//! package has none of them, and the launcher names the package when one
//! is asked for (`packages/ai/package.toml`'s `[provides] commands`).

use crate::provider::{AssetKind, AudioParams, GenerateRequest, ModelParams, TextureParams};
use crate::{register_generated_asset_in, Calls, FlintConfig, JobStore, StyleGuide};
use anyhow::Result;
use clap::Subcommand;
use flint_asset::{AssetCatalog, AssetType, Project, ProjectStore};
use flint_package::cli;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// A human line: stdout, or stderr when stdout is the `--json` document.
macro_rules! say {
    ($($t:tt)*) => { flint_package::cli::say(format_args!($($t)*)) };
}

/// A line about work in flight: always stderr.
macro_rules! progress {
    ($($t:tt)*) => { flint_package::cli::progress(format_args!($($t)*)) };
}

/// `flint asset <...>`, the ai package's part.
#[derive(Subcommand)]
pub enum AiAssetCommands {
    /// Generate an asset using AI
    Generate {
        /// Asset type to generate: texture, model, audio
        asset_type: String,

        /// Description / prompt for generation
        #[arg(long, short)]
        description: String,

        /// Asset name
        #[arg(long)]
        name: Option<String>,

        /// Provider to use (flux, meshy, elevenlabs, mock)
        #[arg(long)]
        provider: Option<String>,

        /// Style guide name
        #[arg(long)]
        style: Option<String>,

        /// Image width (textures only)
        #[arg(long, default_value = "1024")]
        width: u32,

        /// Image height (textures only)
        #[arg(long, default_value = "1024")]
        height: u32,

        /// Random seed for reproducibility
        #[arg(long)]
        seed: Option<u64>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,

        /// Output directory (defaults to <project>/.flint/generated)
        #[arg(long)]
        output: Option<String>,

        /// Audio duration in seconds
        #[arg(long)]
        duration: Option<f64>,

        /// Refuse any paid call that could take this run's spend past USD;
        /// checked before each call is sent
        #[arg(long, value_name = "USD", value_parser = parse_usd)]
        max_cost: Option<f64>,
    },

    /// Validate a generated asset against style constraints
    Validate {
        /// Path to the asset file (e.g., model.glb or model.gltf)
        path: String,

        /// Style guide name for constraint checking
        #[arg(long)]
        style: Option<String>,
    },

    /// Generate a build manifest of all generated assets
    Manifest {
        /// Output path for the manifest file
        #[arg(long, default_value = "build/manifest.toml")]
        output: String,

        /// Assets directory to scan (defaults to the project's assets/)
        #[arg(long)]
        assets_dir: Option<String>,
    },

    /// Regenerate an existing asset with new parameters
    Regenerate {
        /// Asset name to regenerate
        name: String,

        /// Random seed for reproducibility
        #[arg(long)]
        seed: Option<u64>,

        /// Provider override
        #[arg(long)]
        provider: Option<String>,

        /// Style guide name
        #[arg(long)]
        style: Option<String>,

        /// Output directory
        #[arg(long)]
        output: Option<String>,

        /// Refuse any paid call that could take this run's spend past USD;
        /// checked before each call is sent
        #[arg(long, value_name = "USD", value_parser = parse_usd)]
        max_cost: Option<f64>,
    },

    /// What AI calls have cost, from the project's ledger
    /// (.flint/ai_costs.jsonl): assets, procgen specs and looks together
    Costs {
        /// Only calls on or after this date (YYYY-MM-DD)
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
    },

    /// Generations that take minutes (Meshy's models): each is a job from
    /// the moment the provider accepts it
    #[command(subcommand)]
    Job(JobCommands),
}

#[derive(Subcommand)]
pub enum JobCommands {
    /// Show status of a generation job
    Status {
        /// Job ID
        id: String,
    },

    /// List all generation jobs
    List,

    /// Finish a job a generate didn't see through (the process stopped, or
    /// the wait timed out): wait for it, download it and register the
    /// asset. Nothing new is paid for
    Resume {
        /// Job ID
        id: String,

        /// Output directory (defaults to <project>/.flint/generated)
        #[arg(long)]
        output: Option<String>,
    },
}

/// `--max-cost`: a dollar amount, `$` optional, at or above 0 (the kernel's
/// `gen` and `render` flags read it with the same words).
fn parse_usd(s: &str) -> std::result::Result<f64, String> {
    let v: f64 = s
        .trim()
        .trim_start_matches('$')
        .parse()
        .map_err(|_| format!("'{s}' isn't an amount in USD (e.g. 0.50)"))?;
    if !v.is_finite() || v < 0.0 {
        return Err(format!("--max-cost must be a non-negative amount, not {s}"));
    }
    Ok(v)
}

/// Runs the subcommand the line chose.
pub fn run(cmd: AiAssetCommands) -> Result<()> {
    match cmd {
        AiAssetCommands::Generate {
            asset_type,
            description,
            name,
            provider,
            style,
            width,
            height,
            seed,
            tags,
            output,
            duration,
            max_cost,
        } => run_generate(Generate {
            asset_type: &asset_type,
            description: &description,
            name,
            provider,
            style_name: style,
            width,
            height,
            seed,
            tags,
            output_dir: output,
            duration,
            max_cost,
            operation: "generate",
        }),
        AiAssetCommands::Validate { path, style } => run_validate(&path, style.as_deref()),
        AiAssetCommands::Manifest { output, assets_dir } => {
            run_manifest(&output, assets_dir.as_deref())
        }
        AiAssetCommands::Regenerate {
            name,
            seed,
            provider,
            style,
            output,
            max_cost,
        } => run_regenerate(&name, seed, provider, style, output, max_cost),
        AiAssetCommands::Costs { since } => {
            super::costs::cost_report(&super::costs::ledger(), since.as_deref())
        }
        AiAssetCommands::Job(job_cmd) => run_job(job_cmd),
    }
}

/// What `flint asset generate` (and `regenerate`) asks for.
struct Generate<'a> {
    asset_type: &'a str,
    description: &'a str,
    name: Option<String>,
    provider: Option<String>,
    style_name: Option<String>,
    width: u32,
    height: u32,
    seed: Option<u64>,
    tags: Option<String>,
    output_dir: Option<String>,
    duration: Option<f64>,
    max_cost: Option<f64>,
    /// The ledger's `operation`.
    operation: &'static str,
}

/// What a project's provider calls go through: its ledger and prices, the
/// run's ceiling, and its job store.
pub fn project_calls(project: &Project, max_cost: Option<f64>) -> Result<Calls> {
    let meter = super::costs::meter_at(&project.root, "asset-gen", max_cost)?;
    Ok(Calls::new(Arc::new(meter)).with_jobs(JobStore::new(project.jobs_dir())))
}

fn run_generate(g: Generate<'_>) -> Result<()> {
    let Generate {
        asset_type,
        description,
        name,
        provider,
        style_name,
        width,
        height,
        seed,
        tags,
        output_dir,
        duration,
        max_cost,
        operation,
    } = g;
    let kind = match asset_type {
        "texture" => AssetKind::Texture,
        "model" => AssetKind::Model,
        "audio" => AssetKind::Audio,
        _ => anyhow::bail!(
            "Unknown asset type '{}'. Use: texture, model, audio",
            asset_type
        ),
    };

    // Derive name from description if not provided
    let asset_name = name.unwrap_or_else(|| {
        description
            .split_whitespace()
            .take(3)
            .collect::<Vec<_>>()
            .join("_")
            .to_lowercase()
            .replace(|c: char| !c.is_alphanumeric() && c != '_', "")
    });

    // Load config
    let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
        providers: HashMap::new(),
        generation: Default::default(),
    });

    // Determine provider
    let provider_name = provider
        .as_deref()
        .unwrap_or_else(|| config.default_provider(kind));

    // Load style guide
    let style = match style_name.as_deref().or_else(|| config.default_style()) {
        Some(s) => match StyleGuide::find(s) {
            Ok(guide) => Some(guide),
            Err(e) => {
                tracing::warn!("Could not load style '{}': {}", s, e);
                None
            }
        },
        None => None,
    };

    // Build request
    let tag_list: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let request = GenerateRequest {
        name: asset_name.clone(),
        description: description.to_string(),
        kind,
        texture_params: if kind == AssetKind::Texture {
            Some(TextureParams {
                width,
                height,
                seed,
                seamless: false,
            })
        } else {
            None
        },
        model_params: if kind == AssetKind::Model {
            Some(ModelParams {
                seed,
                ..Default::default()
            })
        } else {
            None
        },
        audio_params: if kind == AssetKind::Audio {
            Some(AudioParams {
                duration: duration.unwrap_or(3.0),
                seed,
            })
        } else {
            None
        },
        tags: tag_list,
    };

    // Create provider, its calls metered into the project's ledger
    let project = Project::current()?;
    let calls = project_calls(&project, max_cost)?;
    calls.meter.set_context(&asset_name, operation);
    let gen_provider = crate::providers::create_provider(provider_name, &config, &calls)?;

    progress!(
        "Generating {} '{}' via {}...",
        kind,
        asset_name,
        provider_name
    );

    if let Some(ref s) = style {
        progress!(
            "  Style: {} (prompt enriched with palette + constraints)",
            s.name
        );
    }

    // Show the prompt that will be used
    let prompt = gen_provider.build_prompt(&request, style.as_ref());
    progress!("  Prompt: {}", prompt);
    let estimate = calls
        .meter
        .price_call(provider_name, gen_provider.billed_units(&request))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if estimate > 0.0 {
        progress!("  Cost: up to {}", flint_ai_core::cost::usd(estimate));
    }

    // Generate
    let out_dir = match output_dir.as_deref() {
        Some(d) => std::path::PathBuf::from(d),
        None => project.generated_dir(),
    };

    let result = match gen_provider.generate(&request, style.as_ref(), &out_dir) {
        Ok(r) => r,
        Err(e) => {
            if calls.meter.calls() > 0 {
                progress!("  {}", super::costs::spent_line(&calls.meter));
            }
            return Err(e.into());
        }
    };

    say!("  Downloaded: {}", result.output_path);
    if let Some(job) = result.metadata.get("job_id") {
        say!("  Job: {job}");
    }
    say!("  {}", super::costs::spent_line(&calls.meter));

    // Post-generation validation for models
    let mut checks = None;
    if kind == AssetKind::Model {
        progress!("  Validating...");
        match crate::validate::validate_model(Path::new(&result.output_path), style.as_ref()) {
            Ok(report) => {
                checks = Some(report.checks.clone());
                for check in &report.checks {
                    say!(
                        "    [{:>5}] {}: {}",
                        check.status.to_string(),
                        check.name,
                        check.detail
                    );
                }
            }
            Err(e) => {
                tracing::warn!("Validation skipped: {}", e);
            }
        }
    }

    // Store in content-addressed storage and register sidecar metadata
    let registered = register_generated_asset_in(&project, &request, &result)?;
    say!(
        "  Stored: {} in {}",
        registered.hash,
        Project::display(&project.store_dir())
    );
    say!("  Sidecar: {}", registered.sidecar_path.display());

    say!("  Done in {:.1}s", result.duration_secs);
    cli::when_json(|| {
        json!({
            "name": asset_name,
            "kind": kind.to_string(),
            "provider": provider_name,
            "style": style.as_ref().map(|s| s.name.clone()),
            "prompt": prompt,
            "output": result.output_path,
            "hash": registered.hash.to_string(),
            "sidecar": registered.sidecar_path.display().to_string().replace('\\', "/"),
            "seconds": result.duration_secs,
            "checks": checks,
            "cost_usd": calls.meter.spent(),
            "job": result.metadata.get("job_id"),
        })
    });
    Ok(())
}

fn run_manifest(output: &str, assets_dir: Option<&str>) -> Result<()> {
    let assets_dir = match assets_dir {
        Some(d) => std::path::PathBuf::from(d),
        None => Project::current()?.catalog_dir(),
    };
    let manifest = crate::manifest::BuildManifest::from_assets_directory(&assets_dir)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    let output_path = Path::new(output);
    manifest
        .save(output_path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    say!(
        "Build manifest: {} entries -> {}",
        manifest.entries.len(),
        output
    );
    for entry in &manifest.entries {
        say!(
            "  {} ({}) via {} [{}]",
            entry.name,
            entry.asset_type,
            entry.provider,
            entry.content_hash
        );
    }
    cli::when_json(|| json!({ "output": output, "entries": manifest.entries }));
    Ok(())
}

fn run_regenerate(
    name: &str,
    seed: Option<u64>,
    provider_override: Option<String>,
    style_name: Option<String>,
    output_dir: Option<String>,
    max_cost: Option<f64>,
) -> Result<()> {
    // Look up the existing asset in catalog for its metadata
    let catalog = AssetCatalog::load_from_directory(Project::current()?.catalog_dir())?;
    let meta = catalog
        .get(name)
        .ok_or_else(|| anyhow::anyhow!("Asset '{}' not found in catalog", name))?;

    // Extract original generation info from properties
    let original_prompt = meta
        .properties
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let original_provider = meta
        .properties
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("mock")
        .to_string();

    // Determine asset kind from type
    let kind = match meta.asset_type {
        AssetType::Texture => AssetKind::Texture,
        AssetType::Mesh => AssetKind::Model,
        AssetType::Audio => AssetKind::Audio,
        _ => anyhow::bail!("Cannot regenerate asset type: {:?}", meta.asset_type),
    };

    let provider_name = provider_override.as_deref().unwrap_or(&original_provider);

    progress!(
        "Regenerating '{}' ({}) via {}...",
        name,
        kind,
        provider_name
    );
    if !original_prompt.is_empty() {
        progress!("  Original prompt: {}", original_prompt);
    }

    // Use the same description/prompt, with optional new seed
    run_generate(Generate {
        asset_type: match kind {
            AssetKind::Texture => "texture",
            AssetKind::Model => "model",
            AssetKind::Audio => "audio",
        },
        description: &original_prompt,
        name: Some(name.to_string()),
        provider: Some(provider_name.to_string()),
        style_name,
        width: 1024,
        height: 1024,
        seed,
        tags: None,
        output_dir,
        duration: None,
        max_cost,
        operation: "regenerate",
    })
}

fn run_job(cmd: JobCommands) -> Result<()> {
    let store = JobStore::default_store()?;

    match cmd {
        JobCommands::Status { id } => {
            let job = store.load(&id).map_err(|e| anyhow::anyhow!("{}", e))?;

            say!("Job: {}", job.id);
            say!("  Provider: {}", job.provider);
            say!("  Asset: {}", job.asset_name);
            say!("  Status: {:?}", job.status);
            say!("  Progress: {}%", job.progress);
            if let Some(ref task) = job.remote_id {
                say!("  Task: {}", task);
            }
            if let Some(ref refine) = job.refine_id {
                say!("  Refine: {} (of the task above)", refine);
            }
            say!("  Submitted: {}", job.submitted_at);
            if let Some(ref prompt) = job.prompt {
                say!("  Prompt: {}", prompt);
            }
            if let Some(ref err) = job.error {
                say!("  Error: {}", err);
            }
            if let Some(ref path) = job.output_path {
                say!("  Output: {}", path);
            }
            cli::when_json(|| serde_json::to_value(&job).unwrap_or_default());
            Ok(())
        }
        JobCommands::List => {
            let jobs = store.list().map_err(|e| anyhow::anyhow!("{}", e))?;

            cli::when_json(|| json!({ "jobs": jobs }));
            if jobs.is_empty() {
                say!("No generation jobs found.");
                return Ok(());
            }

            say!("{} job(s):\n", jobs.len());
            for job in &jobs {
                say!(
                    "  {} ({}) {} {:?} {}%{}  {}",
                    job.id,
                    job.provider,
                    job.asset_name,
                    job.status,
                    job.progress,
                    job.estimated_usd
                        .map(|c| format!(" {}", flint_ai_core::cost::usd(c)))
                        .unwrap_or_default(),
                    job.submitted_at
                );
            }
            Ok(())
        }
        JobCommands::Resume { id, output } => run_job_resume(&store, &id, output.as_deref()),
    }
}

fn run_job_resume(store: &JobStore, id: &str, output: Option<&str>) -> Result<()> {
    let job = store.load(id).map_err(|e| anyhow::anyhow!("{}", e))?;
    if job.status == crate::JobStatus::Complete {
        if let Some(path) = job.output_path.as_deref().filter(|p| Path::new(p).exists()) {
            say!("Job {} is already complete: {}", job.id, path);
            cli::when_json(|| serde_json::to_value(&job).unwrap_or_default());
            return Ok(());
        }
    }
    let project = Project::current()?;
    let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
        providers: HashMap::new(),
        generation: Default::default(),
    });
    let calls = project_calls(&project, None)?.with_jobs(store.clone());
    // A resume may pay for a stage the run didn't reach (Meshy's refine of a
    // paid preview, #1187): its ledger line names the asset.
    calls.meter.set_context(&job.asset_name, "resume");
    let provider = crate::providers::create_provider(&job.provider, &config, &calls)?;
    let out_dir = match output {
        Some(d) => std::path::PathBuf::from(d),
        None => project.generated_dir(),
    };
    progress!(
        "Resuming {} job {} for '{}' ({:?}, {}%)...",
        job.provider,
        job.id,
        job.asset_name,
        job.status,
        job.progress
    );
    let resumed = provider.resume(&job, &out_dir);
    if calls.meter.calls() > 0 {
        progress!("  {}", super::costs::spent_line(&calls.meter));
    }
    let result = resumed?;
    let kind = provider
        .supported_kinds()
        .first()
        .copied()
        .unwrap_or(AssetKind::Model);
    let request = GenerateRequest {
        name: job.asset_name.clone(),
        description: job.prompt.clone().unwrap_or_default(),
        kind,
        texture_params: None,
        model_params: (kind == AssetKind::Model).then(ModelParams::default),
        audio_params: None,
        tags: vec![],
    };
    let registered = register_generated_asset_in(&project, &request, &result)?;
    say!("  Downloaded: {}", result.output_path);
    say!("  Stored: {}", registered.hash);
    say!("  Sidecar: {}", registered.sidecar_path.display());
    let job = store.load(id).unwrap_or(job);
    cli::when_json(|| {
        json!({
            "job": job,
            "output": result.output_path,
            "hash": registered.hash.to_string(),
            "sidecar": registered.sidecar_path.display().to_string().replace('\\', "/"),
        })
    });
    Ok(())
}

fn run_validate(path: &str, style_name: Option<&str>) -> Result<()> {
    let asset_path = Path::new(path);
    if !asset_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    let style = match style_name {
        Some(s) => Some(StyleGuide::find(s).map_err(|e| anyhow::anyhow!("{}", e))?),
        None => None,
    };

    let report = crate::validate::validate_model(asset_path, style.as_ref())
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    cli::emit(
        &cli::Answer::new(
            report.summary_text(),
            json!({
                "path": report.path,
                "passed": report.passed,
                "meshes": report.mesh_count,
                "materials": report.material_count,
                "triangles": report.total_triangles,
                "checks": report.checks,
            }),
        )
        .failed_if(!report.passed),
    )
}
