//! `flint edit <file>` — Unified entrypoint that detects the file type and
//! routes to the appropriate editor/viewer.

use std::path::Path;

use anyhow::{bail, Context, Result};

use super::{gen_preview, particle_edit, preview, spline_edit, terrain_edit, tex_edit};

/// Recognised file kinds that `flint edit` can route to.
pub enum FileKind {
    Scene,
    ProcGenSpec,
    TexturePipeline,
    TerrainSpec,
    Model,
    ParticleEffect,
}

/// Inspect the file extension (and TOML content for `.procgen.toml`) to decide
/// which editor to launch.
pub fn detect_file_kind(path: &Path) -> Result<FileKind> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    if name.ends_with(".scene.toml") || name.ends_with(".chunk.toml") {
        return Ok(FileKind::Scene);
    }
    if name.ends_with(".terrain.toml") {
        return Ok(FileKind::TerrainSpec);
    }
    // Suffix only — the file may not exist yet (the editor bootstraps a preset).
    if name.ends_with(".particles.toml") {
        return Ok(FileKind::ParticleEffect);
    }
    if name.ends_with(".procgen.toml") {
        // Disambiguate: pipeline pattern → tex-edit, everything else → gen-preview
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let value: toml::Value = toml::from_str(&content)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let is_pipeline = value
            .get("params")
            .and_then(|p| p.get("pattern"))
            .and_then(|v| v.as_str())
            == Some("pipeline");
        return if is_pipeline {
            Ok(FileKind::TexturePipeline)
        } else {
            Ok(FileKind::ProcGenSpec)
        };
    }

    match path.extension().and_then(|e| e.to_str()) {
        Some("glb" | "gltf") => Ok(FileKind::Model),
        _ => bail!(
            "Cannot determine editor for '{}'\n\
             \n  Supported file types:\n\
             \n    .scene.toml    Scene viewer\
             \n    .procgen.toml  Procgen previewer / texture pipeline editor\
             \n    .terrain.toml  Terrain editor\
             \n    .particles.toml Particle effect editor\
             \n    .glb / .gltf   Model previewer\n",
            path.display()
        ),
    }
}

/// CLI arguments for the unified `flint edit` command.
#[derive(clap::Args)]
pub struct EditArgs {
    /// Path to file (.scene.toml, .procgen.toml, .terrain.toml, .particles.toml, .glb, .gltf)
    pub file: String,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Window width in pixels
    #[arg(long)]
    pub width: Option<u32>,

    /// Window height in pixels
    #[arg(long)]
    pub height: Option<u32>,

    /// Disable the ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Watch for file changes
    #[arg(long)]
    pub watch: bool,

    /// Override seed
    #[arg(long)]
    pub seed: Option<u64>,
    /// Hide the egui inspector panels (scene only)
    #[arg(long)]
    pub no_inspector: bool,

    /// Open the spline/track editor (scene only)
    #[arg(long)]
    pub spline: bool,

    /// Start with auto-orbit enabled (scene/model/procgen)
    #[arg(long)]
    pub auto_orbit: bool,

    /// Camera orbit distance (model, particles)
    #[arg(long)]
    pub distance: Option<f32>,

    /// Camera horizontal angle in degrees (model, particles)
    #[arg(long)]
    pub yaw: Option<f32>,

    /// Camera vertical angle in degrees (model, particles)
    #[arg(long)]
    pub pitch: Option<f32>,

    /// Camera look-at point as comma-separated x,y,z (model, particles)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees (model, particles)
    #[arg(long)]
    pub fov: Option<f32>,

    /// Disable animation playback (model)
    #[arg(long)]
    pub no_animate: bool,

    /// Start with a specific animation clip by name (model)
    #[arg(long)]
    pub clip: Option<String>,

    /// Animation playback speed multiplier (model)
    #[arg(long)]
    pub anim_speed: Option<f32>,

    /// Add an animation layer `clip[:weight[:mask[:mode]]]` (model, repeatable)
    #[arg(long = "layer")]
    pub layers: Vec<String>,

    /// Play a `*.sequence.toml` of timestamped animator events (model)
    #[arg(long)]
    pub sequence: Option<String>,

    /// Sample animation / particle simulation at a time in seconds (model, particles; with --render)
    #[arg(long)]
    pub anim_time: Option<f32>,

    /// Models: turn a joint in its own frame, `joint:x,y,z` in degrees (repeatable)
    #[arg(long = "pose-bone")]
    pub pose_bones: Vec<String>,

    /// Models: draw the skeleton over the model, posed joints in their own colour
    #[arg(long)]
    pub show_skeleton: bool,

    /// Loop the --sequence regardless of its `loop` setting (model)
    #[arg(long)]
    pub sequence_loop: bool,

    /// Render to a PNG file instead of opening a window (model, particles)
    #[arg(long)]
    pub render: Option<String>,

    /// Preset for a new .particles.toml that does not exist yet:
    /// fire, smoke, sparks or rain (particles)
    #[arg(long)]
    pub preset: Option<String>,

    /// `--style name[:key=value,...]` and `--style-time` (the scene viewer,
    /// kept through hot reloads; model and particles with --render)
    #[command(flatten)]
    pub style: crate::commands::common_args::StyleArgs,
}

pub fn run(args: EditArgs) -> Result<()> {
    let path = Path::new(&args.file);
    let file_kind = detect_file_kind(path)?;
    let styled = !args.style.styles.is_empty() || args.style.style_time.is_some();
    let renders =
        args.render.is_some() && matches!(file_kind, FileKind::Model | FileKind::ParticleEffect);
    // The scene viewer takes them too (#328), but not the spline editor
    let scene_viewer = matches!(file_kind, FileKind::Scene) && !args.spline;
    if styled && !renders && !scene_viewer {
        eprintln!(
            "warning: --style / --style-time apply to the scene viewer and to \
             `--render` of a model or a .particles.toml; this editor ignores them \
             (`flint render --style` takes a scene)"
        );
    }

    match file_kind {
        FileKind::Scene => {
            if args.spline {
                // Delegate to the spline/track editor
                spline_edit::run(spline_edit::EditArgs {
                    scene: args.file,
                    schemas: args.schemas,
                })
            } else {
                flint_viewer::app::run(
                    &args.file,
                    args.watch,
                    &args.schemas,
                    !args.no_inspector,
                    args.auto_orbit,
                    flint_viewer::app::ViewerStyle {
                        specs: args.style.styles.clone(),
                        time: args.style.style_time,
                    },
                )
            }
        }
        FileKind::ProcGenSpec => {
            let width = args.width.unwrap_or(1440);
            let height = args.height.unwrap_or(900);
            gen_preview::run(gen_preview::GenPreviewArgs {
                spec: args.file,
                seed: args.seed,
                width,
                height,
                no_grid: args.no_grid,
                auto_orbit: args.auto_orbit,
            })
        }
        FileKind::TexturePipeline => {
            let width = args.width.unwrap_or(1600);
            let height = args.height.unwrap_or(1000);
            tex_edit::run(tex_edit::TexEditArgs {
                spec: args.file,
                seed: args.seed,
                width,
                height,
            })
        }
        FileKind::TerrainSpec => {
            let width = args.width.unwrap_or(1440);
            let height = args.height.unwrap_or(900);
            terrain_edit::run(terrain_edit::TerrainEditArgs {
                spec: args.file,
                seed: args.seed,
                width,
                height,
                no_grid: args.no_grid,
            })
        }
        FileKind::ParticleEffect => {
            let width = args.width.unwrap_or(1500);
            let height = args.height.unwrap_or(940);
            particle_edit::run(particle_edit::ParticleEditArgs {
                file: args.file,
                width,
                height,
                no_grid: args.no_grid,
                auto_orbit: args.auto_orbit,
                render: args.render,
                anim_time: args.anim_time,
                preset: args.preset,
                distance: args.distance,
                yaw: args.yaw,
                pitch: args.pitch,
                target: args.target,
                fov: args.fov,
                style: args.style,
            })
        }
        FileKind::Model => {
            let width = args.width.unwrap_or(1280);
            let height = args.height.unwrap_or(720);
            preview::run(preview::PreviewArgs {
                model: Some(args.file),
                render: args.render,
                width,
                height,
                distance: args.distance,
                yaw: args.yaw,
                pitch: args.pitch,
                target: args.target,
                fov: args.fov,
                no_grid: args.no_grid,
                watch: args.watch,
                no_animate: args.no_animate,
                clip: args.clip,
                anim_speed: args.anim_speed.unwrap_or(1.0),
                anim_time: args.anim_time,
                layers: args.layers,
                sequence: args.sequence,
                sequence_loop: args.sequence_loop,
                auto_orbit: args.auto_orbit,
                style: args.style,
                pose_bones: args.pose_bones,
                show_skeleton: args.show_skeleton,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use std::path::{Path, PathBuf};

    fn fixture(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/tmp/edit_router")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        for (schemas, component) in [("a", "alpha"), ("b", "beta")] {
            let components = dir.join(schemas).join("components");
            std::fs::create_dir_all(&components).unwrap();
            std::fs::write(
                components.join(format!("{component}.toml")),
                format!(
                    "[component.{component}]
[component.{component}.fields]
on = {{ type = \"bool\", default = true }}
"
                ),
            )
            .unwrap();
        }
        std::fs::write(
            dir.join("x.scene.toml"),
            "[scene]
name = \"x\"
",
        )
        .unwrap();
        dir
    }

    /// #704: `flint edit <scene> --schemas a --schemas b` gave the viewer
    /// `a` alone. Parsed through the real command line, on a thread with
    /// the main thread's stack (debug clap is hungry on Windows), then
    /// loaded as `flint_viewer::app::run` loads, short of the window.
    #[test]
    fn the_scene_viewer_gets_every_schemas_flag() {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(|| {
                let dir = fixture("every_flag");
                let cli = crate::Cli::try_parse_from([
                    "flint".as_ref(),
                    "edit".as_ref(),
                    dir.join("x.scene.toml").as_os_str(),
                    "--schemas".as_ref(),
                    dir.join("a").as_os_str(),
                    "--schemas".as_ref(),
                    dir.join("b").as_os_str(),
                ])
                .unwrap_or_else(|e| panic!("{e}"));
                let crate::Commands::Edit(args) = cli.command else {
                    panic!("not an edit command");
                };
                let (registry, _, dirs) =
                    flint_viewer::app::load_schemas(&args.file, &args.schemas).unwrap();
                assert!(registry.get_component("alpha").is_some(), "a's component");
                assert!(
                    registry.get_component("beta").is_some(),
                    "b's component: the viewer was given only {:?}",
                    dirs.dirs
                );
                assert_eq!(dirs.source, flint_schema::SchemaSource::Flags);
            })
            .unwrap()
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
