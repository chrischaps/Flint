//! Asset management commands

use crate::output::{self, progress, say};
use anyhow::Result;
use clap::Subcommand;
use flint_asset::{
    AssetCatalog, AssetMeta, AssetResolver, AssetType, ContentStore, Project, ProjectStore,
    ResolutionStrategy,
};
use flint_asset_gen::provider::{
    AssetKind, AudioParams, GenerateRequest, ModelParams, TextureParams,
};
use flint_asset_gen::{
    register_generated_asset_in, write_asset_sidecar_with_root, Calls, FlintConfig, JobStore,
    StyleGuide,
};
use flint_core::components as comp;
use flint_import::import_gltf;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Subcommand)]
pub enum AssetCommands {
    /// Import an asset file
    Import {
        /// Path to the asset file (e.g., model.glb or model.gltf)
        path: String,

        /// Asset name (defaults to filename without extension)
        #[arg(long)]
        name: Option<String>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },

    /// List registered assets
    List {
        /// Filter by asset type (mesh, texture, material, audio, script)
        #[arg(long, rename_all = "lowercase")]
        r#type: Option<String>,

        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,

        /// Output format (json or toml)
        #[arg(long, default_value = "toml")]
        format: String,
    },

    /// Show asset info
    Info {
        /// Asset name
        name: String,
    },

    /// Resolve asset references in a scene
    Resolve {
        /// Path to scene file
        scene: String,

        /// Resolution strategy (strict, placeholder, ai_generate, human_task, ai_then_human)
        #[arg(long, default_value = "strict")]
        strategy: String,

        /// Style guide name for AI generation
        #[arg(long)]
        style: Option<String>,

        /// Output directory for generated assets or task files
        #[arg(long)]
        output_dir: Option<String>,

        /// Refuse any paid call that could take this run's spend past USD;
        /// checked before each call is sent
        #[arg(long, value_name = "USD", value_parser = super::ai_costs::parse_usd)]
        max_cost: Option<f64>,
    },

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
        #[arg(long, value_name = "USD", value_parser = super::ai_costs::parse_usd)]
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
        #[arg(long, value_name = "USD", value_parser = super::ai_costs::parse_usd)]
        max_cost: Option<f64>,
    },

    /// What AI calls have cost, from the project's ledger
    /// (.flint/ai_costs.jsonl): assets, procgen specs and looks together
    Costs {
        /// Only calls on or after this date (YYYY-MM-DD)
        #[arg(long, value_name = "DATE")]
        since: Option<String>,
    },

    /// Show which file the engine would load for an asset name, and
    /// everywhere it looked first
    #[command(after_help = super::asset_where::kinds_help())]
    Where {
        /// The kind of asset (see the list below)
        #[arg(value_parser = super::asset_where::parse_kind)]
        kind: flint_core::project::AssetKind,

        /// The name a scene would use (`fire`)
        name: String,

        /// The scene that names it; without one, a scene in the working
        /// directory is assumed (the global --json answers as one document)
        #[arg(long)]
        scene: Option<String>,
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

pub fn run(cmd: AssetCommands) -> Result<()> {
    match cmd {
        AssetCommands::Import { path, name, tags } => run_import(&path, name, tags),
        AssetCommands::List {
            r#type,
            tag,
            format,
        } => run_list(r#type, tag, &format),
        AssetCommands::Info { name } => run_info(&name),
        AssetCommands::Resolve {
            scene,
            strategy,
            style,
            output_dir,
            max_cost,
        } => run_resolve(
            &scene,
            &strategy,
            style.as_deref(),
            output_dir.as_deref(),
            max_cost,
        ),
        AssetCommands::Generate {
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
        AssetCommands::Validate { path, style } => run_validate(&path, style.as_deref()),
        AssetCommands::Manifest { output, assets_dir } => {
            run_manifest(&output, assets_dir.as_deref())
        }
        AssetCommands::Regenerate {
            name,
            seed,
            provider,
            style,
            output,
            max_cost,
        } => run_regenerate(&name, seed, provider, style, output, max_cost),
        AssetCommands::Costs { since } => {
            super::ai_costs::cost_report(&super::ai_costs::ledger(), since.as_deref())
        }
        AssetCommands::Where { kind, name, scene } => {
            super::asset_where::run(kind, &name, scene.as_deref(), output::json())
        }
        AssetCommands::Job(job_cmd) => run_job(job_cmd),
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
fn project_calls(project: &Project, max_cost: Option<f64>) -> Result<Calls> {
    let meter = super::ai_costs::meter_at(&project.root, "asset-gen", max_cost)?;
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
    let gen_provider = flint_asset_gen::providers::create_provider(provider_name, &config, &calls)?;

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
                progress!("  {}", super::ai_costs::spent_line(&calls.meter));
            }
            return Err(e.into());
        }
    };

    say!("  Downloaded: {}", result.output_path);
    if let Some(job) = result.metadata.get("job_id") {
        say!("  Job: {job}");
    }
    say!("  {}", super::ai_costs::spent_line(&calls.meter));

    // Post-generation validation for models
    let mut checks = None;
    if kind == AssetKind::Model {
        progress!("  Validating...");
        match flint_asset_gen::validate::validate_model(
            Path::new(&result.output_path),
            style.as_ref(),
        ) {
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
    output::when_json(|| {
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
    let manifest = flint_asset_gen::manifest::BuildManifest::from_assets_directory(&assets_dir)
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
    output::when_json(|| json!({ "output": output, "entries": manifest.entries }));
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
            output::when_json(|| serde_json::to_value(&job).unwrap_or_default());
            Ok(())
        }
        JobCommands::List => {
            let jobs = store.list().map_err(|e| anyhow::anyhow!("{}", e))?;

            output::when_json(|| json!({ "jobs": jobs }));
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
    if job.status == flint_asset_gen::JobStatus::Complete {
        if let Some(path) = job.output_path.as_deref().filter(|p| Path::new(p).exists()) {
            say!("Job {} is already complete: {}", job.id, path);
            output::when_json(|| serde_json::to_value(&job).unwrap_or_default());
            return Ok(());
        }
    }
    let project = Project::current()?;
    let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
        providers: HashMap::new(),
        generation: Default::default(),
    });
    let calls = project_calls(&project, None)?.with_jobs(store.clone());
    let provider = flint_asset_gen::providers::create_provider(&job.provider, &config, &calls)?;
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
    let result = provider.resume(&job, &out_dir)?;
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
    output::when_json(|| {
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

    let report = flint_asset_gen::validate::validate_model(asset_path, style.as_ref())
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    output::emit(
        &output::Answer::new(
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

fn run_import(path: &str, name: Option<String>, tags: Option<String>) -> Result<()> {
    let source_path = Path::new(path);
    if !source_path.exists() {
        anyhow::bail!("File not found: {}", path);
    }

    let ext = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Import based on file type
    let mut meta = match ext.as_str() {
        "glb" | "gltf" => {
            let result = import_gltf(path)?;
            say!(
                "Imported: {} mesh(es), {} texture(s), {} material(s)",
                result.meshes.len(),
                result.textures.len(),
                result.materials.len()
            );
            result.asset_meta
        }
        _ => {
            // Generic import — just hash and store
            let hash = flint_core::ContentHash::from_file(source_path)?;
            AssetMeta {
                name: source_path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unnamed")
                    .to_string(),
                asset_type: guess_asset_type(&ext),
                hash: hash.to_prefixed_hex(),
                source_path: Some(path.to_string()),
                format: Some(ext.clone()),
                properties: HashMap::new(),
                tags: vec![],
            }
        }
    };

    // Override name if provided
    if let Some(n) = name {
        meta.name = n;
    }

    // Add tags
    if let Some(tag_str) = tags {
        meta.tags = tag_str.split(',').map(|t| t.trim().to_string()).collect();
    }

    // Store in the project's content-addressed storage (#179)
    let project = Project::current()?;
    let store = ContentStore::new(project.store_dir());
    store.store(source_path)?;

    // Write sidecar .asset.toml into the project's catalog
    let sidecar_path = write_asset_sidecar_with_root(&meta, &project.catalog_dir())?;

    say!("Asset '{}' registered.", meta.name);
    say!("  Hash: {}", meta.hash);
    say!("  Type: {:?}", meta.asset_type);
    say!("  Stored in: {}", Project::display(&project.store_dir()));
    say!("  Sidecar: {}", sidecar_path.display());
    output::when_json(|| {
        json!({
            "name": meta.name,
            "type": format!("{:?}", meta.asset_type).to_lowercase(),
            "hash": meta.hash,
            "tags": meta.tags,
            "sidecar": sidecar_path.display().to_string().replace('\\', "/"),
        })
    });
    Ok(())
}

fn run_list(type_filter: Option<String>, tag_filter: Option<String>, format: &str) -> Result<()> {
    let catalog_dir = Project::current()?.catalog_dir();
    let catalog = AssetCatalog::load_from_directory(&catalog_dir)?;

    if catalog.is_empty() && !output::json() {
        say!("No assets found in {}", Project::display(&catalog_dir));
        return Ok(());
    }

    let type_enum = type_filter.as_deref().and_then(parse_asset_type);

    let assets: Vec<&AssetMeta> = if let Some(tag) = &tag_filter {
        catalog.by_tag(tag)
    } else if let Some(at) = type_enum {
        catalog.by_type(at)
    } else {
        catalog
            .names()
            .iter()
            .filter_map(|n| catalog.get(n))
            .collect()
    };

    if output::format_or_json(format) == "json" {
        let items: Vec<serde_json::Value> = assets
            .iter()
            .map(|a| {
                serde_json::json!({
                    "name": a.name,
                    "type": format!("{:?}", a.asset_type).to_lowercase(),
                    "hash": a.hash,
                    "tags": a.tags,
                })
            })
            .collect();
        output::document(&serde_json::Value::Array(items));
    } else {
        say!("{} asset(s):\n", assets.len());
        for asset in &assets {
            say!(
                "  {} ({:?}) [{}]",
                asset.name,
                asset.asset_type,
                asset.tags.join(", ")
            );
        }
    }

    Ok(())
}

fn run_info(name: &str) -> Result<()> {
    let catalog = AssetCatalog::load_from_directory(Project::current()?.catalog_dir())?;

    match catalog.get(name) {
        Some(meta) => {
            output::when_json(|| {
                json!({
                    "name": meta.name,
                    "type": format!("{:?}", meta.asset_type).to_lowercase(),
                    "hash": meta.hash,
                    "source": meta.source_path,
                    "format": meta.format,
                    "tags": meta.tags,
                    "properties": serde_json::to_value(&meta.properties).unwrap_or_default(),
                })
            });
            say!("Asset: {}", meta.name);
            say!("  Type: {:?}", meta.asset_type);
            say!("  Hash: {}", meta.hash);
            if let Some(ref source) = meta.source_path {
                say!("  Source: {}", source);
            }
            if let Some(ref fmt) = meta.format {
                say!("  Format: {}", fmt);
            }
            if !meta.tags.is_empty() {
                say!("  Tags: {}", meta.tags.join(", "));
            }
            if !meta.properties.is_empty() {
                say!("  Properties:");
                for (key, value) in &meta.properties {
                    say!("    {}: {}", key, value);
                }
            }
        }
        None => {
            anyhow::bail!("Asset '{}' not found", name);
        }
    }

    Ok(())
}

fn run_resolve(
    scene_path: &str,
    strategy_str: &str,
    style_name: Option<&str>,
    output_dir: Option<&str>,
    max_cost: Option<f64>,
) -> Result<()> {
    // Check for batch strategies (ai_generate, human_task, ai_then_human)
    let batch_strategy = match strategy_str {
        "ai_generate" => Some(flint_asset_gen::batch::BatchStrategy::AiGenerate),
        "human_task" => Some(flint_asset_gen::batch::BatchStrategy::HumanTask),
        "ai_then_human" => Some(flint_asset_gen::batch::BatchStrategy::AiThenHuman),
        _ => None,
    };

    if let Some(batch_strat) = batch_strategy {
        let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
            providers: HashMap::new(),
            generation: Default::default(),
        });

        let style = match style_name.or_else(|| config.default_style()) {
            Some(s) => StyleGuide::find(s).ok(),
            None => None,
        };

        let project = Project::for_path(Path::new(scene_path))?;
        let generated = project.generated_dir().display().to_string();
        let out = output_dir.unwrap_or(&generated);
        // One meter for the whole scene: the scene's project's ledger and
        // prices, and the run's ceiling.
        let calls = project_calls(&project, max_cost)?;

        let result = flint_asset_gen::batch::resolve_scene(
            scene_path,
            batch_strat,
            style.as_ref(),
            &config,
            out,
            &calls,
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;
        progress!("{}", super::ai_costs::spent_line(&calls.meter));

        output::when_json(|| {
            json!({
                "scene": scene_path,
                "strategy": strategy_str,
                "references": result.total_refs,
                "found": result.already_found,
                "generated": result.generated,
                "tasks_created": result.tasks_created,
                "failed": result.failed,
                "over_budget": result.over_budget,
                "cost_usd": result.spent_usd,
                "max_cost": max_cost,
            })
        });
        // A reference neither generated nor handed to a person is a failure
        let unresolved =
            result.total_refs - result.already_found - result.generated - result.tasks_created;
        if result.failed > 0 || unresolved > 0 {
            return Err(output::failed());
        }
        return Ok(());
    }

    // Existing catalog-based resolution
    let strategy = match strategy_str {
        "strict" => ResolutionStrategy::Strict,
        "placeholder" => ResolutionStrategy::Placeholder,
        _ => anyhow::bail!(
            "Unknown strategy: {} (use 'strict', 'placeholder', 'ai_generate', 'human_task', 'ai_then_human')",
            strategy_str
        ),
    };

    // The scene's project's catalog, not the working directory's (#179).
    let catalog =
        AssetCatalog::load_from_directory(Project::for_path(Path::new(scene_path))?.catalog_dir())?;
    let resolver = AssetResolver::new(strategy);

    // Load and scan scene for asset references
    let content = std::fs::read_to_string(scene_path)?;
    let scene: toml::Value = toml::from_str(&content)?;

    let mut found = 0usize;
    let mut missing = 0usize;
    let mut missing_refs = Vec::new();

    for reference in collect_scene_asset_references(&scene) {
        let asset_ref = flint_asset::AssetRef::ByName(reference.name.clone());
        let result = resolver.resolve(&asset_ref, &catalog);
        if result.is_found() {
            found += 1;
        } else {
            missing += 1;
            missing_refs.push(json!({
                "entity": reference.entity,
                "field": reference.field,
                "name": reference.name,
            }));
            say!(
                "  Missing: {}.{} -> \"{}\"",
                reference.entity,
                reference.field,
                reference.name
            );
        }
    }

    say!("\nResolution: {} found, {} missing", found, missing);
    let failed = missing > 0 && strategy == ResolutionStrategy::Strict;
    output::when_json(|| {
        json!({
            "scene": scene_path,
            "strategy": strategy_str,
            "found": found,
            "missing": missing_refs,
            "valid": !failed,
        })
    });

    if failed {
        return Err(output::failed());
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct SceneAssetReference {
    entity: String,
    field: String,
    name: String,
}

fn collect_scene_asset_references(scene: &toml::Value) -> Vec<SceneAssetReference> {
    let mut refs = Vec::new();

    let Some(entities) = scene.get("entities").and_then(|e| e.as_table()) else {
        return refs;
    };

    for (entity_name, entity_data) in entities {
        let Some(table) = entity_data.as_table() else {
            continue;
        };

        // Legacy top-level references
        for field in ["mesh", "texture", "material", "audio", "script"] {
            push_ref_if_string(table, entity_name, field, field, &mut refs);
        }

        // Current component references
        if let Some(model) = table.get(comp::MODEL).and_then(|v| v.as_table()) {
            push_ref_if_string(model, entity_name, "asset", "model.asset", &mut refs);
        }

        if let Some(material) = table.get(comp::MATERIAL).and_then(|v| v.as_table()) {
            push_ref_if_string(
                material,
                entity_name,
                "texture",
                "material.texture",
                &mut refs,
            );
        }

        if let Some(sprite) = table.get(comp::SPRITE).and_then(|v| v.as_table()) {
            push_ref_if_string(sprite, entity_name, "texture", "sprite.texture", &mut refs);
        }

        if let Some(audio) = table.get(comp::AUDIO_SOURCE).and_then(|v| v.as_table()) {
            push_ref_if_string(audio, entity_name, "file", "audio_source.file", &mut refs);
        }
    }

    refs
}

fn push_ref_if_string(
    table: &toml::map::Map<String, toml::Value>,
    entity_name: &str,
    key: &str,
    field_name: &str,
    refs: &mut Vec<SceneAssetReference>,
) {
    if let Some(name) = table.get(key).and_then(|v| v.as_str()) {
        if !name.is_empty() {
            refs.push(SceneAssetReference {
                entity: entity_name.to_string(),
                field: field_name.to_string(),
                name: name.to_string(),
            });
        }
    }
}

fn guess_asset_type(ext: &str) -> AssetType {
    match ext {
        "glb" | "gltf" | "obj" | "fbx" => AssetType::Mesh,
        "png" | "jpg" | "jpeg" | "bmp" | "tga" | "hdr" => AssetType::Texture,
        "wav" | "ogg" | "mp3" | "flac" => AssetType::Audio,
        "rhai" | "lua" | "wasm" => AssetType::Script,
        _ => AssetType::Mesh,
    }
}

fn parse_asset_type(s: &str) -> Option<AssetType> {
    match s.to_lowercase().as_str() {
        "mesh" => Some(AssetType::Mesh),
        "texture" => Some(AssetType::Texture),
        "material" => Some(AssetType::Material),
        "audio" => Some(AssetType::Audio),
        "script" => Some(AssetType::Script),
        _ => None,
    }
}
