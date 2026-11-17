//! `flint edit <file>` — Unified entrypoint that detects the file type and
//! routes to the appropriate editor/viewer.

use std::path::Path;

use anyhow::{bail, Context, Result};
use flint_viewer::host::{self, CameraFlags, Editor, HostOptions, StillOptions};

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

/// The scene viewer's window from `--width` / `--height` / `--no-grid`
/// (#423: the viewer took none of them and opened 1600 x 900 with the grid).
/// A size left out keeps the viewer's default for that side.
fn scene_window(args: &EditArgs) -> flint_viewer::app::ViewerWindow {
    let default = flint_viewer::app::ViewerWindow::default();
    flint_viewer::app::ViewerWindow {
        width: args.width.unwrap_or(default.width),
        height: args.height.unwrap_or(default.height),
        grid: !args.no_grid,
    }
}

/// CLI arguments for the unified `flint edit` command. A flag the file's
/// editor doesn't take is an error (#712; `flag_kinds` is the table).
#[derive(clap::Args)]
pub struct EditArgs {
    /// Path to file (.scene.toml, .procgen.toml, .terrain.toml, .particles.toml, .glb, .gltf)
    pub file: String,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Window (or --render still) width in pixels
    #[arg(long)]
    pub width: Option<u32>,

    /// Window (or --render still) height in pixels
    #[arg(long)]
    pub height: Option<u32>,

    /// Disable the ground grid (editors with a 3D view)
    #[arg(long)]
    pub no_grid: bool,

    /// Watch for file changes
    #[arg(long)]
    pub watch: bool,

    /// Override the seed (procgen specs, texture pipelines, terrain)
    #[arg(long)]
    pub seed: Option<u64>,
    /// Hide the egui inspector panels (scene only)
    #[arg(long)]
    pub no_inspector: bool,

    /// Open the spline/track editor (scene only)
    #[arg(long)]
    pub spline: bool,

    /// Start with auto-orbit enabled (editors with a 3D view, but the track editor)
    #[arg(long)]
    pub auto_orbit: bool,

    /// Camera orbit distance (editors with a 3D view)
    #[arg(long)]
    pub distance: Option<f32>,

    /// Camera horizontal angle in degrees (editors with a 3D view)
    #[arg(long)]
    pub yaw: Option<f32>,

    /// Camera vertical angle in degrees (editors with a 3D view)
    #[arg(long)]
    pub pitch: Option<f32>,

    /// Camera look-at point as comma-separated x,y,z (editors with a 3D view)
    #[arg(long, value_parser = crate::commands::common_args::parse_vec3)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees (editors with a 3D view)
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

    /// Render to a PNG file instead of opening a window (every editor: its
    /// view without the panels; a texture's images side by side)
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

    /// Motion on twos for the whole frame (#871): drawn positions and poses
    /// change only this many times a second (12 = on twos at 24 fps; 0 = off),
    /// over the scene's `[post_process] motion_hold`. The scene viewer and the
    /// model previewer
    #[arg(long, value_name = "RATE")]
    pub motion_hold: Option<f32>,
}

/// The editor a file opens, as finely as the flags care (#712): a
/// procgen spec draws a mesh or images, and a scene opens the viewer or,
/// with `--spline`, the track editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorKind {
    Scene,
    Track,
    Mesh,
    Texture,
    TextureGraph,
    Terrain,
    Model,
    Particles,
}

use EditorKind as K;

impl EditorKind {
    /// What `--render` and the flag errors call it.
    pub fn label(self) -> &'static str {
        match self {
            K::Scene => "a scene (the scene viewer)",
            K::Track => "a scene with --spline (the track editor)",
            K::Mesh => "a procgen mesh spec",
            K::Texture => "a procgen texture spec",
            K::TextureGraph => "a texture pipeline spec",
            K::Terrain => "a terrain spec",
            K::Model => "a model (.glb / .gltf)",
            K::Particles => "a particle effect",
        }
    }

    /// The window's size when `--width` / `--height` leave it out.
    fn default_size(self) -> (u32, u32) {
        match self {
            K::Scene | K::Track => {
                let window = flint_viewer::app::ViewerWindow::default();
                (window.width, window.height)
            }
            K::Mesh | K::Texture | K::Terrain => (1440, 900),
            K::TextureGraph => (1600, 1000),
            K::Model => (1280, 720),
            K::Particles => (1500, 940),
        }
    }
}

/// The editors with a 3D view: the camera flags and `--no-grid` apply.
const VIEWS: &[EditorKind] = &[
    K::Scene,
    K::Track,
    K::Mesh,
    K::Terrain,
    K::Model,
    K::Particles,
];
/// The editors that generate from a seed.
const SEEDED: &[EditorKind] = &[K::Mesh, K::Texture, K::TextureGraph, K::Terrain];
const MODEL: &[EditorKind] = &[K::Model];

/// Every flag `flint edit` takes that not every editor does, with the
/// editors it applies to. One table, so a flag means the same wherever it
/// is taken and is an error wherever it isn't (#712: they were ignored).
/// `--width`, `--height`, `--watch` and `--render` apply to every editor.
fn flag_kinds(args: &EditArgs) -> Vec<(&'static str, &'static [EditorKind])> {
    let mut given: Vec<(&'static str, &'static [EditorKind])> = Vec::new();
    let mut flag = |on: bool, name: &'static str, kinds: &'static [EditorKind]| {
        if on {
            given.push((name, kinds));
        }
    };
    flag(!args.schemas.is_empty(), "--schemas", &[K::Scene, K::Track]);
    flag(args.no_grid, "--no-grid", VIEWS);
    flag(args.distance.is_some(), "--distance", VIEWS);
    flag(args.yaw.is_some(), "--yaw", VIEWS);
    flag(args.pitch.is_some(), "--pitch", VIEWS);
    flag(args.target.is_some(), "--target", VIEWS);
    flag(args.fov.is_some(), "--fov", VIEWS);
    flag(
        args.auto_orbit,
        "--auto-orbit",
        &[K::Scene, K::Mesh, K::Terrain, K::Model, K::Particles],
    );
    flag(args.seed.is_some(), "--seed", SEEDED);
    flag(args.no_inspector, "--no-inspector", &[K::Scene]);
    flag(args.no_animate, "--no-animate", MODEL);
    flag(args.clip.is_some(), "--clip", MODEL);
    flag(args.anim_speed.is_some(), "--anim-speed", MODEL);
    flag(!args.layers.is_empty(), "--layer", MODEL);
    flag(args.sequence.is_some(), "--sequence", MODEL);
    flag(args.sequence_loop, "--sequence-loop", MODEL);
    flag(!args.pose_bones.is_empty(), "--pose-bone", MODEL);
    flag(args.show_skeleton, "--show-skeleton", MODEL);
    flag(
        args.anim_time.is_some(),
        "--anim-time",
        &[K::Model, K::Particles],
    );
    flag(args.preset.is_some(), "--preset", &[K::Particles]);
    flag(
        !args.style.styles.is_empty(),
        "--style",
        &[K::Scene, K::Model, K::Particles],
    );
    flag(
        args.style.style_time.is_some(),
        "--style-time",
        &[K::Scene, K::Model, K::Particles],
    );
    flag(
        args.motion_hold.is_some(),
        "--motion-hold",
        &[K::Scene, K::Model],
    );
    given
}

/// The flags given that `kind` doesn't take, each naming what it applies
/// to; and the ones that are only a still's.
fn check_flags(args: &EditArgs, kind: EditorKind) -> Result<()> {
    let mut problems: Vec<String> = flag_kinds(args)
        .into_iter()
        .filter(|(_, kinds)| !kinds.contains(&kind))
        .map(|(name, kinds)| {
            let to: Vec<&str> = kinds.iter().map(|k| k.label()).collect();
            format!(
                "{name} doesn't apply to {}; it applies to {}",
                kind.label(),
                to.join(", ")
            )
        })
        .collect();
    if args.render.is_none() {
        // A window plays these on its own clock; a still is one moment
        if args.anim_time.is_some() && matches!(kind, K::Model | K::Particles) {
            problems.push("--anim-time is a still's moment: give --render too".into());
        }
        let styled = !args.style.styles.is_empty() || args.style.style_time.is_some();
        if styled && matches!(kind, K::Model | K::Particles) {
            problems.push(format!(
                "--style / --style-time on {} apply to its --render still; \
                 the window draws the editor's own view",
                kind.label()
            ));
        }
    }
    if args.spline && kind != K::Track {
        problems.push(format!("--spline applies to a scene, not {}", kind.label()));
    }
    if problems.is_empty() {
        return Ok(());
    }
    bail!("{}", problems.join("\n"))
}

pub fn run(args: EditArgs) -> Result<()> {
    let path = Path::new(&args.file);
    let file_kind = detect_file_kind(path)?;
    let kind = match file_kind {
        FileKind::Scene if args.spline => K::Track,
        FileKind::Scene => K::Scene,
        FileKind::ProcGenSpec => {
            if gen_preview::draws_images(path)? {
                K::Texture
            } else {
                K::Mesh
            }
        }
        FileKind::TexturePipeline => K::TextureGraph,
        FileKind::TerrainSpec => K::Terrain,
        FileKind::Model => K::Model,
        FileKind::ParticleEffect => K::Particles,
    };
    check_flags(&args, kind)?;
    // A window can't answer --json (#710); a still of any kind can (#712)
    if args.render.is_none() {
        crate::output::refuse_interactive("edit")?;
    }

    let (width, height) = kind.default_size();
    let options = HostOptions {
        width: args.width.unwrap_or(width),
        height: args.height.unwrap_or(height),
        camera: CameraFlags {
            distance: args.distance,
            yaw: args.yaw,
            pitch: args.pitch,
            target: args.target,
            fov: args.fov,
        },
        auto_orbit: args.auto_orbit,
    };
    let launch = Launch {
        kind,
        options: options.clone(),
        render: args.render.clone(),
        still: StillOptions {
            time: args.anim_time,
        },
    };
    let window = scene_window(&args);

    match kind {
        K::Track => launch.go(spline_edit::editor(
            spline_edit::EditArgs {
                scene: args.file,
                schemas: args.schemas,
            },
            window,
        )?),
        K::Scene => launch.go(flint_viewer::app::viewer(
            &args.file,
            args.watch,
            &args.schemas,
            !args.no_inspector,
            args.auto_orbit,
            flint_viewer::app::ViewerStyle {
                specs: args.style.styles.clone(),
                time: args.style.style_time,
                motion_hold: args.motion_hold,
            },
            window,
        )?),
        K::Mesh | K::Texture => launch.go(gen_preview::editor(gen_preview::GenPreviewArgs {
            spec: args.file,
            seed: args.seed,
            width: options.width,
            height: options.height,
            no_grid: args.no_grid,
            auto_orbit: args.auto_orbit,
        })?),
        K::TextureGraph => launch.go(tex_edit::editor(tex_edit::TexEditArgs {
            spec: args.file,
            seed: args.seed,
            width: options.width,
            height: options.height,
        })?),
        K::Terrain => launch.go(terrain_edit::editor(terrain_edit::TerrainEditArgs {
            spec: args.file,
            seed: args.seed,
            width: options.width,
            height: options.height,
            no_grid: args.no_grid,
        })?),
        K::Particles => launch.go(particle_edit::editor(particle_edit::ParticleEditArgs {
            file: args.file,
            width: options.width,
            height: options.height,
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
        })?),
        K::Model => launch.go(preview::editor(preview::PreviewArgs {
            model: Some(args.file),
            render: args.render,
            width: options.width,
            height: options.height,
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
            motion_hold: args.motion_hold,
        })?),
    }
}

/// An editor's way out: a window, or with `--render` a still.
pub struct Launch {
    pub kind: EditorKind,
    pub options: HostOptions,
    pub render: Option<String>,
    pub still: StillOptions,
}

impl Launch {
    pub fn go(self, mut editor: impl Editor) -> Result<()> {
        let Some(out) = self.render else {
            return host::run(editor, self.options);
        };
        let picture = host::render_still(&mut editor, &self.options, &self.still)?;
        if let Some(parent) = Path::new(&out).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        picture
            .save(&out)
            .with_context(|| format!("Failed to save image to {out}"))?;
        crate::output::say!(
            "Rendered {}x{} of {} to {out}",
            picture.width(),
            picture.height(),
            self.kind.label()
        );
        if let Some(note) = editor.still_note() {
            crate::output::say!("  {note}");
        }
        if crate::output::json() {
            crate::output::document(&serde_json::json!({ "written": [out] }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{check_flags, scene_window, EditorKind as K};
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

    /// #423: `flint edit <scene> --no-grid --width W --height H` opened a
    /// 1600 x 900 window with the grid. Parsed through the real command
    /// line, on a big-stack thread as above.
    #[test]
    fn the_scene_viewer_gets_its_size_and_grid_flags() {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(|| {
                let window = |words: &[&str]| {
                    let cli = crate::Cli::try_parse_from(
                        ["flint", "edit", "x.scene.toml"].iter().chain(words),
                    )
                    .unwrap_or_else(|e| panic!("{e}"));
                    let crate::Commands::Edit(args) = cli.command else {
                        panic!("not an edit command");
                    };
                    scene_window(&args)
                };
                let given = window(&["--no-grid", "--width", "1280", "--height", "720"]);
                assert_eq!(
                    given,
                    flint_viewer::app::ViewerWindow {
                        width: 1280,
                        height: 720,
                        grid: false,
                    }
                );
                // No flags: the viewer's own default, grid on
                assert_eq!(window(&[]), flint_viewer::app::ViewerWindow::default());
                assert!(window(&[]).grid);
                // One side given keeps the other's default
                let tall = window(&["--height", "1200"]);
                assert_eq!((tall.width, tall.height, tall.grid), (1600, 1200, true));
            })
            .unwrap()
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
    /// #712's first done-when: one `initialize`. Every editor `flint edit`
    /// opens is a `flint_viewer::host::Editor`, and the host is the only
    /// place a window, a render context or egui's window state is made.
    /// The play-chart window (`flint play-chart`) is no editor, and nor is
    /// the frame debugger (`flint frame open`, #1088): it edits nothing,
    /// orbits no camera and watches no file, and its `--screenshot` draws
    /// the egui window itself, where the host's still draws a scene (#1202).
    #[test]
    fn every_editor_opens_through_the_one_host() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let allowed = [
            "flint-viewer/src/host.rs",
            "flint-cli/src/commands/play_chart_window.rs",
            "flint-cli/src/commands/frame/open/host.rs",
        ];
        let mut found = Vec::new();
        let mut stack = vec![root.join("flint-cli/src"), root.join("flint-viewer/src")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = std::fs::read_to_string(&path).unwrap();
                // Split, so this file doesn't name them
                for needle in [
                    concat!("impl Application", "Handler for"),
                    concat!(".create_", "window("),
                    concat!("RenderContext", "::new("),
                    concat!("egui_winit::State", "::new("),
                ] {
                    if text.contains(needle) && !allowed.contains(&rel.as_str()) {
                        found.push(format!("{rel}: {needle}"));
                    }
                }
            }
        }
        assert!(
            found.is_empty(),
            "an editor host outside host.rs: {found:#?}"
        );
    }

    /// The flags every editor takes pass for every kind; the table names
    /// each kind somewhere, so no editor is left without its flags.
    #[test]
    fn every_kind_takes_the_common_flags() {
        std::thread::Builder::new()
            .stack_size(16 << 20)
            .spawn(|| {
                let cli = crate::Cli::try_parse_from([
                    "flint", "edit", "x.glb", "--width", "320", "--height", "200", "--watch",
                    "--render", "x.png",
                ])
                .unwrap_or_else(|e| panic!("{e}"));
                let crate::Commands::Edit(args) = cli.command else {
                    panic!("not an edit command");
                };
                for kind in [
                    K::Scene,
                    K::Mesh,
                    K::Texture,
                    K::TextureGraph,
                    K::Terrain,
                    K::Model,
                    K::Particles,
                ] {
                    check_flags(&args, kind).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
                }
            })
            .unwrap()
            .join()
            .unwrap_or_else(|e| std::panic::resume_unwind(e));
    }
}
