//! Scene management commands

use anyhow::{Context, Result};
use clap::Subcommand;
use flint_scene::SceneFile;
use std::fs;
use std::path::Path;

#[derive(Subcommand)]
pub enum SceneCommands {
    /// Create a new scene file
    Create {
        /// Path to scene file
        path: String,

        /// Scene name (defaults to filename)
        #[arg(long)]
        name: Option<String>,
    },

    /// List all scene files in a directory
    List {
        /// Directory to search (defaults to current directory)
        #[arg(default_value = ".")]
        path: String,
    },

    /// Show scene information
    Info {
        /// Path to scene file
        path: String,
    },
}

pub fn run(cmd: SceneCommands) -> Result<()> {
    match cmd {
        SceneCommands::Create { path, name } => create(&path, name.as_deref()),
        SceneCommands::List { path } => list(&path),
        SceneCommands::Info { path } => info(&path),
    }
}

fn create(path: &str, name: Option<&str>) -> Result<()> {
    // Ensure path ends with .toml or .scene.toml
    let path = if path.ends_with(".toml") {
        path.to_string()
    } else if path.ends_with(".scene") {
        format!("{}.toml", path)
    } else {
        format!("{}.scene.toml", path)
    };

    if Path::new(&path).exists() {
        anyhow::bail!("Scene file already exists: {}", path);
    }

    // Derive name from path if not provided
    let scene_name = name.map(String::from).unwrap_or_else(|| {
        Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.trim_end_matches(".scene"))
            .unwrap_or("Untitled")
            .to_string()
    });

    let scene = SceneFile::new(&scene_name);
    let content = toml::to_string_pretty(&scene)?;

    // Create parent directories if needed
    if let Some(parent) = Path::new(&path).parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(&path, content)?;

    println!("Created scene: {}", path);

    Ok(())
}

fn list(dir: &str) -> Result<()> {
    let path = Path::new(dir);

    if !path.exists() {
        anyhow::bail!("Directory not found: {}", dir);
    }

    if !path.is_dir() {
        anyhow::bail!("Not a directory: {}", dir);
    }

    let mut scenes = Vec::new();

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_path = entry.path();

        if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
            let _name = file_path.file_name().unwrap().to_string_lossy().to_string();

            // Try to parse as scene file
            if let Ok(content) = fs::read_to_string(&file_path) {
                if let Ok(scene) = toml::from_str::<SceneFile>(&content) {
                    scenes.push((file_path.display().to_string(), scene.scene.name));
                }
            }
        }
    }

    // Also check subdirectories (one level deep)
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let subdir = entry.path();

        if subdir.is_dir() {
            for sub_entry in fs::read_dir(&subdir).into_iter().flatten() {
                if let Ok(sub_entry) = sub_entry {
                    let file_path = sub_entry.path();

                    if file_path.extension().map(|e| e == "toml").unwrap_or(false) {
                        if let Ok(content) = fs::read_to_string(&file_path) {
                            if let Ok(scene) = toml::from_str::<SceneFile>(&content) {
                                scenes.push((file_path.display().to_string(), scene.scene.name));
                            }
                        }
                    }
                }
            }
        }
    }

    if scenes.is_empty() {
        println!("No scene files found in {}", dir);
    } else {
        println!("Scene files:");
        for (path, name) in scenes {
            println!("  {} ({})", path, name);
        }
    }

    Ok(())
}

fn info(path: &str) -> Result<()> {
    let content = fs::read_to_string(path).context("Failed to read scene file")?;
    let scene: SceneFile = toml::from_str(&content).context("Failed to parse scene file")?;

    println!("Scene: {}", scene.scene.name);
    println!("Version: {}", scene.scene.version);
    if let Some(desc) = &scene.scene.description {
        println!("Description: {}", desc);
    }
    println!("Entities: {}", scene.entities.len());

    if !scene.entities.is_empty() {
        println!("");
        println!("Entity list:");
        for (name, def) in &scene.entities {
            let archetype = def.archetype.as_deref().unwrap_or("(none)");
            let parent = def.parent.as_deref().map(|p| format!(" -> {}", p)).unwrap_or_default();
            println!("  {} [{}]{}", name, archetype, parent);
        }
    }

    Ok(())
}
