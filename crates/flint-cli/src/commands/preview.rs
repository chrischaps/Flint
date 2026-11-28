//! `flint preview [model]` (hidden) and `flint edit <model>.glb`: the model
//! previewer is the rig package's (#1621, `flint_rig::cli`), opened through
//! the viewer's `ModelEditors` point; this is the command's flags and the
//! open. A flint without the package names it, before the file is read.

use anyhow::{Context, Result};
use flint_package::cli::Points;
use flint_viewer::host::{self, CameraFlags, HostOptions, ModelEdit, ModelEditors, StillOptions};

#[derive(clap::Args)]
pub struct PreviewArgs {
    /// Path to model file (.glb or .gltf). If omitted, opens an empty window for drag-and-drop.
    pub model: Option<String>,

    /// Render to a PNG file instead of opening a window
    #[arg(long)]
    pub render: Option<String>,

    /// Image width in pixels
    #[arg(long, default_value = "1280")]
    pub width: u32,

    /// Image height in pixels
    #[arg(long, default_value = "720")]
    pub height: u32,

    /// Camera orbit distance
    #[arg(long)]
    pub distance: Option<f32>,

    /// Camera horizontal angle in degrees
    #[arg(long, allow_negative_numbers = true)]
    pub yaw: Option<f32>,

    /// Camera vertical angle in degrees
    #[arg(long, allow_negative_numbers = true)]
    pub pitch: Option<f32>,

    /// Camera look-at point (comma-separated x,y,z)
    #[arg(long, value_name = "X,Y,Z", value_parser = crate::commands::common_args::parse_vec3,
          allow_hyphen_values = true)]
    pub target: Option<[f32; 3]>,

    /// Field of view in degrees
    #[arg(long)]
    pub fov: Option<f32>,

    /// Disable ground grid
    #[arg(long)]
    pub no_grid: bool,

    /// Watch model file for changes and auto-reload
    #[arg(long)]
    pub watch: bool,

    /// Disable animation playback (animations play by default when present)
    #[arg(long)]
    pub no_animate: bool,

    /// Start with a specific animation clip by name
    #[arg(long)]
    pub clip: Option<String>,

    /// Animation playback speed multiplier (default: 1.0)
    #[arg(long, default_value = "1.0")]
    pub anim_speed: f32,

    /// Sample animation at a specific time in seconds (headless --render mode only)
    #[arg(long)]
    pub anim_time: Option<f32>,

    /// Add an animation layer: `clip[:weight[:mask[:mode]]]` (repeatable, in order)
    #[arg(long = "layer")]
    pub layers: Vec<String>,

    /// Play a `*.sequence.toml` of timestamped animator events (blend /
    /// layer / speed / cue). With --render, --anim-time samples the
    /// sequence by deterministic replay.
    #[arg(long)]
    pub sequence: Option<String>,

    /// Loop the --sequence regardless of its `loop` setting
    #[arg(long)]
    pub sequence_loop: bool,

    /// Start with auto-orbit enabled
    #[arg(long)]
    pub auto_orbit: bool,

    /// `--style` / `--style-time` for --render (#271)
    #[command(flatten)]
    pub style: crate::commands::common_args::StyleArgs,
    /// Turn a joint in its own frame, `joint:x,y,z` in degrees (repeatable):
    /// a skinned model holds the pose with or without a clip
    #[arg(long = "pose-bone")]
    pub pose_bones: Vec<String>,

    /// Draw the skeleton over the model, posed joints in their own colour
    #[arg(long)]
    pub show_skeleton: bool,

    /// Motion on twos (#871): the pose changes only this many times a second
    /// of the clip's own time, playing and scrubbing (12 = on twos at 24 fps;
    /// 0 = off). The Animation panel's "Hold" switches it live
    #[arg(long, value_name = "RATE")]
    pub motion_hold: Option<f32>,
}

impl PreviewArgs {
    /// What the previewer is handed.
    pub fn edit(self) -> ModelEdit {
        ModelEdit {
            model: self.model,
            render: self.render,
            width: self.width,
            height: self.height,
            distance: self.distance,
            yaw: self.yaw,
            pitch: self.pitch,
            target: self.target,
            fov: self.fov,
            no_grid: self.no_grid,
            watch: self.watch,
            no_animate: self.no_animate,
            clip: self.clip,
            anim_speed: self.anim_speed,
            anim_time: self.anim_time,
            layers: self.layers,
            sequence: self.sequence,
            sequence_loop: self.sequence_loop,
            auto_orbit: self.auto_orbit,
            styles: self.style.styles,
            style_time: self.style.style_time,
            pose_bones: self.pose_bones,
            show_skeleton: self.show_skeleton,
            motion_hold: self.motion_hold,
        }
    }
}

/// The package's model previewer, or the package named: `what` is the
/// command line that wanted it.
pub(crate) fn editors<'p>(points: &'p Points, what: &str) -> Result<&'p ModelEditors> {
    points
        .get::<ModelEditors>()
        .ok_or_else(|| super::absent::missing_owned(what.to_string(), &["preview"]))
}

/// `flint preview [model]`: the previewer in a window, or with `--render`
/// a still through the editor host (#712).
pub fn run(args: PreviewArgs) -> Result<()> {
    run_in(args, crate::points())
}

/// [`run`] against `points`, for a test's own.
pub(crate) fn run_in(args: PreviewArgs, points: &Points) -> Result<()> {
    let editors = editors(points, "flint preview")?;
    let options = HostOptions {
        width: args.width,
        height: args.height,
        camera: CameraFlags {
            distance: args.distance,
            yaw: args.yaw,
            pitch: args.pitch,
            target: args.target,
            fov: args.fov,
        },
        auto_orbit: args.auto_orbit,
    };
    let still = StillOptions {
        time: args.anim_time,
    };
    let output = args.render.clone();
    let mut editor = (editors.open)(args.edit())?;
    let Some(output) = output else {
        return host::run(editor, options);
    };
    let img = host::render_still(&mut editor, &options, &still)?;
    img.save(&output)
        .context(format!("Failed to save image to {}", output))?;
    crate::output::say!(
        "Rendered {}x{} preview to {}",
        img.width(),
        img.height(),
        output
    );
    Ok(())
}
