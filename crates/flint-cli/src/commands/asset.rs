//! Asset management commands: the kernel's part of `flint asset`. The ai
//! package's subcommands (`generate`, `regenerate`, `validate`,
//! `manifest`, `costs`, `job`) are its own extension under this command
//! (`flint_asset_gen::cli`, #1553), listed after these.

#[cfg(feature = "ai")]
use crate::output::progress;
use crate::output::{self, say};
use anyhow::Result;
use clap::Subcommand;
use flint_asset::{
    AssetCatalog, AssetMeta, AssetResolver, AssetType, ContentStore, Project, ProjectStore,
    ResolutionStrategy,
};
#[cfg(feature = "ai")]
use flint_asset_gen::{FlintConfig, StyleGuide};
use flint_import::import_gltf;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;

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
        #[arg(long, value_name = "USD", value_parser = super::common_args::parse_usd)]
        max_cost: Option<f64>,

        /// Schema directory, repeatable; the list of every `asset` field
        /// read comes from these. Without it: the project's flint.toml
        /// `schemas`, else schemas/ in the working directory, beside the
        /// scene and one level up
        #[arg(long)]
        schemas: Vec<String>,
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
}

#[cfg(not(feature = "ai"))]
impl AssetCommands {
    /// The words of an ask that is the ai package's: `resolve`'s batch
    /// strategies (#1333). Its own subcommands are its extension's, so a
    /// build without it has none to ask for.
    pub fn ai_words(&self) -> Option<String> {
        match self {
            AssetCommands::Resolve { strategy, .. } if BATCH_STRATEGIES.contains(&&**strategy) => {
                Some(format!("flint asset resolve --strategy {strategy}"))
            }
            _ => None,
        }
    }
}

/// `resolve`'s strategies that hand a missing asset to a provider or a
/// person, through flint-asset-gen's batch.
const BATCH_STRATEGIES: &[&str] = &["ai_generate", "human_task", "ai_then_human"];

pub fn run(cmd: AssetCommands) -> Result<()> {
    #[cfg(not(feature = "ai"))]
    if let Some(words) = cmd.ai_words() {
        return Err(super::absent::missing(words, "ai"));
    }
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
            schemas,
        } => run_resolve(
            &scene,
            &strategy,
            style.as_deref(),
            output_dir.as_deref(),
            max_cost,
            &schemas,
        ),
        AssetCommands::Where { kind, name, scene } => {
            super::asset_where::run(kind, &name, scene.as_deref(), output::json())
        }
    }
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
    let sidecar_path = write_sidecar(&meta, &project.catalog_dir())?;

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

/// An asset's sidecar, `<catalog>/<kind folder>/<name>.asset.toml`: the
/// file flint-asset-gen's `write_asset_sidecar_with_root` writes for a
/// generated one, written here so `asset import` is the kernel's and needs
/// no ai package (#1333). `tests::an_imports_sidecar_is_the_one_asset_gen_writes`
/// holds the two to the same bytes.
fn write_sidecar(meta: &AssetMeta, catalog: &Path) -> Result<std::path::PathBuf> {
    #[derive(serde::Serialize)]
    struct Sidecar<'a> {
        asset: &'a AssetMeta,
    }
    let folder = match meta.asset_type {
        AssetType::Mesh => "meshes",
        AssetType::Texture => "textures",
        AssetType::Material => "materials",
        AssetType::Audio => "audio",
        AssetType::Script => "scripts",
    };
    let dir = catalog.join(folder);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.asset.toml", meta.name));
    std::fs::write(&path, toml::to_string_pretty(&Sidecar { asset: meta })?)?;
    Ok(path)
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
    schemas: &[String],
) -> Result<()> {
    // Check for batch strategies (ai_generate, human_task, ai_then_human)
    let batch = BATCH_STRATEGIES.contains(&strategy_str);
    let strategy = match strategy_str {
        "strict" => Some(ResolutionStrategy::Strict),
        "placeholder" => Some(ResolutionStrategy::Placeholder),
        _ if batch => None,
        _ => anyhow::bail!(
            "Unknown strategy: {} (use 'strict', 'placeholder', 'ai_generate', 'human_task', 'ai_then_human')",
            strategy_str
        ),
    };

    // The scene's project's catalog, not the working directory's (#179),
    // and every `asset` field its schemas declare (#968).
    let project = Project::for_path(Path::new(scene_path))?;
    let catalog = AssetCatalog::load_from_directory(project.catalog_dir())?;
    let references = scene_asset_references(scene_path, schemas, &project, &catalog)?;
    let found = references.iter().filter(|r| r.found.is_some()).count();
    for reference in &references {
        say!("{}", reference.line());
    }

    #[cfg(feature = "ai")]
    if batch {
        let batch_strat = match strategy_str {
            "ai_generate" => flint_asset_gen::batch::BatchStrategy::AiGenerate,
            "human_task" => flint_asset_gen::batch::BatchStrategy::HumanTask,
            _ => flint_asset_gen::batch::BatchStrategy::AiThenHuman,
        };
        let config = FlintConfig::load().unwrap_or_else(|_| FlintConfig {
            providers: HashMap::new(),
            generation: Default::default(),
        });

        let style = match style_name.or_else(|| config.default_style()) {
            Some(s) => StyleGuide::find(s).ok(),
            None => None,
        };

        let generated = project.generated_dir().display().to_string();
        let out = output_dir.unwrap_or(&generated);
        // One meter for the whole scene: the scene's project's ledger and
        // prices, and the run's ceiling.
        let calls = flint_asset_gen::cli::asset::project_calls(&project, max_cost)?;

        // What a provider makes is a model, a texture or a sound; a missing
        // script or heightmap is reported, and left unresolved
        let mut needs = flint_asset_gen::batch::SceneNeeds::default();
        let mut no_provider = Vec::new();
        for reference in references.iter().filter(|r| r.found.is_none()) {
            match generated_kind(&reference.kinds) {
                Some(kind) => needs.missing.push(flint_asset_gen::batch::MissingAsset {
                    entity_name: reference.place(),
                    field: reference.field(),
                    asset_name: reference.name.clone(),
                    kind,
                }),
                None => no_provider.push(reference),
            }
        }
        needs.total = found + needs.missing.len();
        for reference in &no_provider {
            say!(
                "  No provider makes {} {}: left missing",
                kinds_label(&reference.kinds),
                reference.field_of()
            );
        }

        let result = flint_asset_gen::batch::resolve_missing(
            &project,
            &needs,
            batch_strat,
            style.as_ref(),
            &config,
            out,
            &calls,
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;
        progress!("{}", flint_asset_gen::cli::costs::spent_line(&calls.meter));

        output::when_json(|| {
            json!({
                "scene": scene_path,
                "strategy": strategy_str,
                "references": references.len(),
                "found": result.already_found,
                "generated": result.generated,
                "tasks_created": result.tasks_created,
                "failed": result.failed,
                "over_budget": result.over_budget,
                "no_provider": no_provider.iter().map(|r| r.json()).collect::<Vec<_>>(),
                "cost_usd": result.spent_usd,
                "max_cost": max_cost,
            })
        });
        // A reference neither generated nor handed to a person is a failure
        let unresolved =
            result.total_refs - result.already_found - result.generated - result.tasks_created;
        if result.failed > 0 || unresolved > 0 || !no_provider.is_empty() {
            return Err(output::failed());
        }
        return Ok(());
    }

    // Without the ai package `run` refused a batch strategy before here
    #[cfg(not(feature = "ai"))]
    let _ = (style_name, output_dir, max_cost);
    let strategy = strategy.expect("a catalog strategy");
    let missing: Vec<&SceneAssetReference> =
        references.iter().filter(|r| r.found.is_none()).collect();
    say!("\nResolution: {} found, {} missing", found, missing.len());
    let failed = !missing.is_empty() && strategy == ResolutionStrategy::Strict;
    output::when_json(|| {
        json!({
            "scene": scene_path,
            "strategy": strategy_str,
            "found": found,
            "missing": missing.iter().map(|r| r.json()).collect::<Vec<_>>(),
            "references": references.iter().map(|r| r.json()).collect::<Vec<_>>(),
            "valid": !failed,
        })
    });

    if failed {
        return Err(output::failed());
    }
    Ok(())
}

/// One asset a scene names, and where it was found (#968).
#[derive(Debug, Clone)]
struct SceneAssetReference {
    /// The entity it is written on, or `None` for a scene table.
    entity: Option<String>,
    /// The component, or the scene table.
    table: String,
    /// The field's path inside it.
    path: String,
    name: String,
    kinds: Vec<flint_core::project::AssetKind>,
    /// `catalog`, or the file the engine loads; `None` when nothing holds it.
    found: Option<String>,
    /// Why nothing holds it: every path tried, or that an `asset_def`
    /// declares it and it is not made yet.
    why_missing: Option<String>,
}

impl SceneAssetReference {
    /// `model.asset`, or `environment.skybox` for a scene table.
    fn field(&self) -> String {
        format!("{}.{}", self.table, self.path)
    }

    /// The entity, or `[environment]` for a scene table.
    #[cfg(feature = "ai")]
    fn place(&self) -> String {
        match &self.entity {
            Some(entity) => entity.clone(),
            None => format!("[{}]", self.table),
        }
    }

    /// `crate.model.asset`, or `[environment].skybox`.
    fn field_of(&self) -> String {
        match &self.entity {
            Some(entity) => format!("{entity}.{}", self.field()),
            None => format!("[{}].{}", self.table, self.path),
        }
    }

    fn line(&self) -> String {
        match (&self.found, &self.why_missing) {
            (Some(found), _) => format!(
                "  Found: {} -> \"{}\" ({found})",
                self.field_of(),
                self.name
            ),
            (None, why) => format!(
                "  Missing: {} -> \"{}\"{}",
                self.field_of(),
                self.name,
                why.as_deref()
                    .map(|why| format!(" ({why})"))
                    .unwrap_or_default()
            ),
        }
    }

    fn json(&self) -> serde_json::Value {
        json!({
            "entity": self.entity,
            "table": self.table,
            "field": self.field(),
            "name": self.name,
            "kinds": self.kinds.iter().map(|k| k.label()).collect::<Vec<_>>(),
            "found": self.found,
            "missing": self.why_missing,
        })
    }
}

/// Every asset the scene at `scene_path` names, from its schemas (#968):
/// each field the loaded schemas type `asset`, through arrays and tables,
/// in every entity (prefabs expanded) and scene table, as
/// [`flint_scene::asset_references`] walks them. Each is found in the
/// project's asset catalog by name, else as the engine finds it (a file,
/// or a model or texture in the catalog and store).
fn scene_asset_references(
    scene_path: &str,
    schemas: &[String],
    project: &Project,
    catalog: &AssetCatalog,
) -> Result<Vec<SceneAssetReference>> {
    let (registry, _) = super::common_args::load_schemas(Some(scene_path), schemas)?;
    // With no schema nothing is known to name an asset, and a report of
    // nothing found would read as a scene with nothing to make
    if registry.component_names().is_empty() {
        anyhow::bail!(
            "no component schemas were found for {scene_path}, so no field is known to name \
             an asset; pass --schemas <dir> (the engine's is `schemas/`, a game's \
             `engine/schemas`) or list them in flint.toml's [project] schemas"
        );
    }
    let resolver = AssetResolver::new(ResolutionStrategy::Strict);
    let walked = flint_scene::asset_references(Path::new(scene_path), &registry, project)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(walked
        .into_iter()
        .map(|r| {
            let catalogued = resolver
                .resolve(&flint_asset::AssetRef::ByName(r.name.clone()), catalog)
                .is_found();
            let (found, why_missing) = match r.located {
                _ if catalogued => (Some("catalog".to_string()), None),
                flint_scene::Located::File(path) => (Some(flint_core::project::shown(&path)), None),
                flint_scene::Located::Catalog => (Some("catalog".to_string()), None),
                flint_scene::Located::Declared => (
                    None,
                    Some("an asset_def declares it; not made yet".to_string()),
                ),
                flint_scene::Located::Missing { tried } => (None, Some(tried)),
            };
            SceneAssetReference {
                entity: r.entity,
                table: r.table,
                path: r.field,
                name: r.name,
                kinds: r.kinds,
                found,
                why_missing,
            }
        })
        .collect())
}

/// What a provider can make for a field of `kinds`: a model, a texture or
/// a sound, the first the field takes.
#[cfg(feature = "ai")]
fn generated_kind(
    kinds: &[flint_core::project::AssetKind],
) -> Option<flint_asset_gen::provider::AssetKind> {
    use flint_asset_gen::provider::AssetKind as Made;
    use flint_core::project::AssetKind as Kind;
    kinds.iter().find_map(|kind| match kind {
        Kind::Models => Some(Made::Model),
        Kind::Textures | Kind::ParticleTextures | Kind::Sprites => Some(Made::Texture),
        Kind::Audio => Some(Made::Audio),
        _ => None,
    })
}

/// `files`, or `models or specs`.
#[cfg(feature = "ai")]
fn kinds_label(kinds: &[flint_core::project::AssetKind]) -> String {
    let labels: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
    labels.join(" or ")
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
