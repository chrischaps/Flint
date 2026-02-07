//! Asset management commands

use anyhow::Result;
use clap::Subcommand;
use flint_asset::{AssetCatalog, AssetMeta, AssetResolver, AssetType, ContentStore, ResolutionStrategy};
use flint_asset_gen::provider::{AssetKind, AudioParams, GenerateRequest, ModelParams, TextureParams};
use flint_asset_gen::{register_generated_asset, write_asset_sidecar, FlintConfig, JobStore, StyleGuide};
use flint_import::import_gltf;
use std::collections::HashMap;
use std::path::Path;

#[derive(Subcommand)]
pub enum AssetCommands {
    /// Import an asset file
    Import {
        /// Path to the asset file (e.g., model.glb)
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

        /// Output directory (defaults to .flint/generated)
        #[arg(long)]
        output: Option<String>,

        /// Audio duration in seconds
        #[arg(long)]
        duration: Option<f64>,
    },

    /// Validate a generated asset against style constraints
    Validate {
        /// Path to the asset file (e.g., model.glb)
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

        /// Assets directory to scan
        #[arg(long, default_value = "assets")]
        assets_dir: String,
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
    },

    /// Manage generation jobs
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
        } => run_resolve(&scene, &strategy, style.as_deref(), output_dir.as_deref()),
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
        } => run_generate(
            &asset_type,
            &description,
            name,
            provider,
            style,
            width,
            height,
            seed,
            tags,
            output,
            duration,
        ),
        AssetCommands::Validate { path, style } => run_validate(&path, style.as_deref()),
        AssetCommands::Manifest { output, assets_dir } => run_manifest(&output, &assets_dir),
        AssetCommands::Regenerate {
            name,
            seed,
            provider,
            style,
            output,
        } => run_regenerate(&name, seed, provider, style, output),
        AssetCommands::Job(job_cmd) => run_job(job_cmd),
    }
}

fn run_generate(
    asset_type: &str,
    description: &str,
    name: Option<String>,
    provider: Option<String>,
    style_name: Option<String>,
    width: u32,
    height: u32,
    seed: Option<u64>,
    tags: Option<String>,
    output_dir: Option<String>,
    duration: Option<f64>,
) -> Result<()> {
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
    let style = match style_name
        .as_deref()
        .or_else(|| config.default_style())
    {
        Some(s) => match StyleGuide::find(s) {
            Ok(guide) => Some(guide),
            Err(e) => {
                eprintln!("Warning: Could not load style '{}': {}", s, e);
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

    // Create provider
    let gen_provider = flint_asset_gen::providers::create_provider(provider_name, &config)?;

    println!(
        "Generating {} '{}' via {}...",
        kind, asset_name, provider_name
    );

    if let Some(ref s) = style {
        println!("  Style: {} (prompt enriched with palette + constraints)", s.name);
    }

    // Show the prompt that will be used
    let prompt = gen_provider.build_prompt(&request, style.as_ref());
    println!("  Prompt: {}", prompt);

    // Generate
    let out_dir = output_dir
        .as_deref()
        .unwrap_or(".flint/generated");

    let result = gen_provider.generate(&request, style.as_ref(), Path::new(out_dir))?;

    println!("  Downloaded: {}", result.output_path);

    // Post-generation validation for models
    if kind == AssetKind::Model {
        println!("  Validating...");
        match flint_asset_gen::validate::validate_model(
            Path::new(&result.output_path),
            style.as_ref(),
        ) {
            Ok(report) => {
                for check in &report.checks {
                    let icon = match check.status {
                        flint_asset_gen::validate::CheckStatus::Pass => "OK",
                        flint_asset_gen::validate::CheckStatus::Warn => "WARN",
                        flint_asset_gen::validate::CheckStatus::Fail => "FAIL",
                    };
                    println!("    {}: {}  {}", check.name, check.detail, icon);
                }
            }
            Err(e) => {
                eprintln!("  Validation skipped: {}", e);
            }
        }
    }

    // Store in content-addressed storage and register sidecar metadata
    let registered = register_generated_asset(&request, &result)?;
    println!("  Stored: {}", registered.hash);
    println!("  Sidecar: {}", registered.sidecar_path.display());

    println!("  Done in {:.1}s", result.duration_secs);
    Ok(())
}

fn run_manifest(output: &str, assets_dir: &str) -> Result<()> {
    let manifest =
        flint_asset_gen::manifest::BuildManifest::from_assets_directory(Path::new(assets_dir))
            .map_err(|e| anyhow::anyhow!("{}", e))?;

    let output_path = Path::new(output);
    manifest
        .save(output_path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    println!(
        "Build manifest: {} entries -> {}",
        manifest.entries.len(),
        output
    );
    for entry in &manifest.entries {
        println!(
            "  {} ({}) via {} [{}]",
            entry.name, entry.asset_type, entry.provider, entry.content_hash
        );
    }
    Ok(())
}

fn run_regenerate(
    name: &str,
    seed: Option<u64>,
    provider_override: Option<String>,
    style_name: Option<String>,
    output_dir: Option<String>,
) -> Result<()> {
    // Look up the existing asset in catalog for its metadata
    let catalog = AssetCatalog::load_from_directory("assets")?;
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

    let provider_name = provider_override
        .as_deref()
        .unwrap_or(&original_provider);

    println!(
        "Regenerating '{}' ({}) via {}...",
        name, kind, provider_name
    );
    if !original_prompt.is_empty() {
        println!("  Original prompt: {}", original_prompt);
    }

    // Use the same description/prompt, with optional new seed
    run_generate(
        match kind {
            AssetKind::Texture => "texture",
            AssetKind::Model => "model",
            AssetKind::Audio => "audio",
        },
        &original_prompt,
        Some(name.to_string()),
        Some(provider_name.to_string()),
        style_name,
        1024,
        1024,
        seed,
        None,
        output_dir,
        None,
    )
}

fn run_job(cmd: JobCommands) -> Result<()> {
    let store = JobStore::default_store();

    match cmd {
        JobCommands::Status { id } => {
            let job = store
                .load(&id)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            println!("Job: {}", job.id);
            println!("  Provider: {}", job.provider);
            println!("  Asset: {}", job.asset_name);
            println!("  Status: {:?}", job.status);
            println!("  Progress: {}%", job.progress);
            println!("  Submitted: {}", job.submitted_at);
            if let Some(ref prompt) = job.prompt {
                println!("  Prompt: {}", prompt);
            }
            if let Some(ref err) = job.error {
                println!("  Error: {}", err);
            }
            if let Some(ref path) = job.output_path {
                println!("  Output: {}", path);
            }
            Ok(())
        }
        JobCommands::List => {
            let jobs = store.list().map_err(|e| anyhow::anyhow!("{}", e))?;

            if jobs.is_empty() {
                println!("No generation jobs found.");
                return Ok(());
            }

            println!("{} job(s):\n", jobs.len());
            for job in &jobs {
                println!(
                    "  {} ({}) {} {:?} {}%",
                    job.id, job.provider, job.asset_name, job.status, job.progress
                );
            }
            Ok(())
        }
    }
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

    report.print_summary();

    if !report.passed {
        std::process::exit(1);
    }

    Ok(())
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
            println!(
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

    // Store in content-addressed storage
    let store = ContentStore::new(".flint/assets");
    store.store(source_path)?;

    // Write sidecar .asset.toml
    let sidecar_path = write_asset_sidecar(&meta)?;

    println!("Asset '{}' registered.", meta.name);
    println!("  Hash: {}", meta.hash);
    println!("  Type: {:?}", meta.asset_type);
    println!("  Sidecar: {}", sidecar_path.display());

    Ok(())
}

fn run_list(type_filter: Option<String>, tag_filter: Option<String>, format: &str) -> Result<()> {
    let catalog = AssetCatalog::load_from_directory("assets")?;

    if catalog.is_empty() {
        println!("No assets found in assets/");
        return Ok(());
    }

    let type_enum = type_filter.as_deref().and_then(parse_asset_type);

    let assets: Vec<&AssetMeta> = if let Some(tag) = &tag_filter {
        catalog.by_tag(tag)
    } else if let Some(at) = type_enum {
        catalog.by_type(at)
    } else {
        catalog.names().iter().filter_map(|n| catalog.get(n)).collect()
    };

    if format == "json" {
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
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        println!("{} asset(s):\n", assets.len());
        for asset in &assets {
            println!(
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
    let catalog = AssetCatalog::load_from_directory("assets")?;

    match catalog.get(name) {
        Some(meta) => {
            println!("Asset: {}", meta.name);
            println!("  Type: {:?}", meta.asset_type);
            println!("  Hash: {}", meta.hash);
            if let Some(ref source) = meta.source_path {
                println!("  Source: {}", source);
            }
            if let Some(ref fmt) = meta.format {
                println!("  Format: {}", fmt);
            }
            if !meta.tags.is_empty() {
                println!("  Tags: {}", meta.tags.join(", "));
            }
            if !meta.properties.is_empty() {
                println!("  Properties:");
                for (key, value) in &meta.properties {
                    println!("    {}: {}", key, value);
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

        let out = output_dir.unwrap_or(".flint/generated");

        flint_asset_gen::batch::resolve_scene(
            scene_path,
            batch_strat,
            style.as_ref(),
            &config,
            out,
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;

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

    let catalog = AssetCatalog::load_from_directory("assets")?;
    let resolver = AssetResolver::new(strategy);

    // Load and scan scene for asset references
    let content = std::fs::read_to_string(scene_path)?;
    let scene: toml::Value = toml::from_str(&content)?;

    let mut found = 0usize;
    let mut missing = 0usize;

    for reference in collect_scene_asset_references(&scene) {
        let asset_ref = flint_asset::AssetRef::ByName(reference.name.clone());
        let result = resolver.resolve(&asset_ref, &catalog);
        if result.is_found() {
            found += 1;
        } else {
            missing += 1;
            println!(
                "  Missing: {}.{} -> \"{}\"",
                reference.entity,
                reference.field,
                reference.name
            );
        }
    }

    println!("\nResolution: {} found, {} missing", found, missing);

    if missing > 0 && strategy == ResolutionStrategy::Strict {
        std::process::exit(1);
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
        if let Some(model) = table.get("model").and_then(|v| v.as_table()) {
            push_ref_if_string(model, entity_name, "asset", "model.asset", &mut refs);
        }

        if let Some(material) = table.get("material").and_then(|v| v.as_table()) {
            push_ref_if_string(
                material,
                entity_name,
                "texture",
                "material.texture",
                &mut refs,
            );
        }

        if let Some(sprite) = table.get("sprite").and_then(|v| v.as_table()) {
            push_ref_if_string(sprite, entity_name, "texture", "sprite.texture", &mut refs);
        }

        if let Some(audio) = table.get("audio_source").and_then(|v| v.as_table()) {
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
