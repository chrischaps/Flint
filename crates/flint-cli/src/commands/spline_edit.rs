//! Track editor — loads a scene with spline data and launches the interactive editor.

use anyhow::{Context, Result};
use flint_core::toml_util::toml_f32;
use flint_scene::load_scene;
use flint_viewer::spline_editor::{ControlPoint, SplineEditorConfig};
use std::path::Path;

#[derive(clap::Args)]
pub struct EditArgs {
    /// Path to scene file
    pub scene: String,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

pub fn run(args: EditArgs) -> Result<()> {
    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let (registry, schema_dirs) =
        super::common_args::load_schemas(Some(&args.scene), &args.schemas)?;
    super::common_args::warn_dropped_schemas(&schema_dirs);
    if schema_dirs.dirs.is_empty() {
        println!("Warning: No schemas directories found");
    }

    // Load scene
    let (world, scene_file) = load_scene(&args.scene, &registry).context("Failed to load scene")?;
    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    // Find the first entity with a `spline` component
    let scene_dir = Path::new(&args.scene)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut spline_file_path: Option<String> = None;
    let mut spline_name = String::from("Track");

    for entity in world.all_entities() {
        let spline_file = world
            .get_components(entity.id)
            .and_then(|c| c.get("spline").cloned())
            .and_then(|s| s.get("file").and_then(|v| v.as_str().map(String::from)));

        if let Some(file_rel) = spline_file {
            // Resolve path: scene dir, then parent
            let full_path = {
                let p = scene_dir.join(&file_rel);
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    let pp = parent.join(&file_rel);
                    if pp.exists() {
                        pp
                    } else {
                        p
                    }
                } else {
                    p
                }
            };

            if full_path.exists() {
                spline_file_path = Some(full_path.to_string_lossy().to_string());
                // Try to get a name from the entity
                spline_name = entity.name.clone();
                println!("Found spline: {} -> {}", entity.name, full_path.display());
                break;
            } else {
                tracing::warn!("Spline file not found: {}", full_path.display());
            }
        }
    }

    let spline_path =
        spline_file_path.context("No entity with a `spline` component found in the scene")?;

    // Parse control points from the spline TOML file
    let content = std::fs::read_to_string(&spline_path)
        .with_context(|| format!("Failed to read spline file: {}", spline_path))?;
    let value: toml::Value =
        toml::from_str(&content).with_context(|| "Failed to parse spline TOML")?;

    let spline_section = value
        .get("spline")
        .context("Missing [spline] section in spline file")?;
    let closed = spline_section
        .get("closed")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let name = spline_section
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or(&spline_name)
        .to_string();

    let spacing = value
        .get("sampling")
        .and_then(|s| s.get("spacing"))
        .and_then(toml_f32)
        .unwrap_or(2.0);

    let points_arr = value
        .get("control_points")
        .and_then(|v| v.as_array())
        .context("Missing [[control_points]] in spline file")?;

    let mut control_points = Vec::new();
    for pt in points_arr {
        let pos = pt
            .get("position")
            .and_then(|v| v.as_array())
            .context("Control point missing position")?;
        if pos.len() < 3 {
            continue;
        }
        let x = toml_f32(&pos[0]).unwrap_or(0.0);
        let y = toml_f32(&pos[1]).unwrap_or(0.0);
        let z = toml_f32(&pos[2]).unwrap_or(0.0);
        let twist = pt.get("twist").and_then(toml_f32).unwrap_or(0.0);

        control_points.push(ControlPoint {
            position: [x, y, z],
            twist,
        });
    }

    println!(
        "Loaded {} control points from {} (closed={}, spacing={:.1})",
        control_points.len(),
        spline_path,
        closed,
        spacing
    );

    // Build editor config
    let editor_config = SplineEditorConfig {
        control_points,
        closed,
        spacing,
        name,
        file_path: spline_path,
    };

    // Launch viewer in editor mode with file watching enabled
    flint_viewer::app::run_editor(&args.scene, true, &args.schemas, editor_config)
}
