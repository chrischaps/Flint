//! Asset management commands

use anyhow::Result;
use clap::Subcommand;
use flint_asset::{AssetCatalog, AssetMeta, AssetResolver, AssetType, ContentStore, ResolutionStrategy};
use flint_import::import_gltf;
use std::collections::HashMap;
use std::fs;
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

        /// Resolution strategy (strict or placeholder)
        #[arg(long, default_value = "strict")]
        strategy: String,
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
        AssetCommands::Resolve { scene, strategy } => run_resolve(&scene, &strategy),
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

    // Determine asset subdirectory
    let subdir = match meta.asset_type {
        AssetType::Mesh => "meshes",
        AssetType::Texture => "textures",
        AssetType::Material => "materials",
        AssetType::Audio => "audio",
        AssetType::Script => "scripts",
    };

    // Write sidecar .asset.toml
    let assets_dir = Path::new("assets").join(subdir);
    fs::create_dir_all(&assets_dir)?;

    let sidecar_path = assets_dir.join(format!("{}.asset.toml", meta.name));
    let sidecar_content = format_asset_toml(&meta);
    fs::write(&sidecar_path, sidecar_content)?;

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

fn run_resolve(scene_path: &str, strategy_str: &str) -> Result<()> {
    let strategy = match strategy_str {
        "strict" => ResolutionStrategy::Strict,
        "placeholder" => ResolutionStrategy::Placeholder,
        _ => anyhow::bail!("Unknown strategy: {} (use 'strict' or 'placeholder')", strategy_str),
    };

    let catalog = AssetCatalog::load_from_directory("assets")?;
    let resolver = AssetResolver::new(strategy);

    // Load and scan scene for asset references
    let content = std::fs::read_to_string(scene_path)?;
    let scene: toml::Value = toml::from_str(&content)?;

    let mut found = 0;
    let mut missing = 0;

    if let Some(entities) = scene.get("entities").and_then(|e| e.as_table()) {
        for (entity_name, entity_data) in entities {
            if let Some(table) = entity_data.as_table() {
                // Check for common asset reference fields
                for field in &["mesh", "texture", "material", "audio", "script"] {
                    if let Some(value) = table.get(*field) {
                        if let Some(name) = value.as_str() {
                            let asset_ref = flint_asset::AssetRef::ByName(name.to_string());
                            let result = resolver.resolve(&asset_ref, &catalog);
                            if result.is_found() {
                                found += 1;
                            } else {
                                missing += 1;
                                println!(
                                    "  Missing: {}.{} -> \"{}\"",
                                    entity_name, field, name
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    println!("\nResolution: {} found, {} missing", found, missing);

    if missing > 0 && strategy == ResolutionStrategy::Strict {
        std::process::exit(1);
    }

    Ok(())
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

fn format_asset_toml(meta: &AssetMeta) -> String {
    let mut out = String::new();
    out.push_str("[asset]\n");
    out.push_str(&format!("name = \"{}\"\n", meta.name));
    out.push_str(&format!(
        "type = \"{}\"\n",
        format!("{:?}", meta.asset_type).to_lowercase()
    ));
    out.push_str(&format!("hash = \"{}\"\n", meta.hash));

    if let Some(ref source) = meta.source_path {
        out.push_str(&format!("source_path = \"{}\"\n", source));
    }
    if let Some(ref fmt) = meta.format {
        out.push_str(&format!("format = \"{}\"\n", fmt));
    }

    if !meta.tags.is_empty() {
        let tags: Vec<String> = meta.tags.iter().map(|t| format!("\"{}\"", t)).collect();
        out.push_str(&format!("tags = [{}]\n", tags.join(", ")));
    }

    if !meta.properties.is_empty() {
        out.push_str("\n[asset.properties]\n");
        for (key, value) in &meta.properties {
            out.push_str(&format!("{} = {}\n", key, value));
        }
    }

    out
}
