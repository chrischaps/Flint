//! `flint render --frame-dump <file.json>` (#1084) and `--frame-capture
//! <dir>` (#1085): the rendered frame's description as JSON, the type a
//! capture's `manifest.json` holds (ADR 0126, `flint_render::frame_capture`),
//! and with a capture every pass's output image beside it.
//!
//! [`arm`] goes before the frame is drawn and [`write`] after it is read
//! back. While armed the renderer logs its passes (and, capturing, copies
//! each target as its pass leaves it); the GPU timer is on for that one
//! frame (where the adapter can time) and keeps its raw times, so each pass
//! carries what it cost in that frame, not a smoothed figure. A render
//! without either flag never arms it and draws as it always did.

use anyhow::{Context, Result};
use flint_render::{HeadlessContext, SceneRenderer};

/// Which of the frame's records a render asked for.
#[derive(Clone, Copy)]
pub(super) struct Wanted<'a> {
    /// `--frame-dump`: the description as JSON.
    pub dump: Option<&'a str>,
    /// `--frame-capture`: the folder for the manifest and the images.
    pub capture: Option<&'a str>,
}

impl<'a> Wanted<'a> {
    pub fn of(args: &'a super::render::RenderArgs) -> Option<Self> {
        let wanted = Self {
            dump: args.measure.frame_dump.as_deref(),
            capture: args.measure.frame_capture.as_deref(),
        };
        (wanted.dump.is_some() || wanted.capture.is_some()).then_some(wanted)
    }
}

/// Where the frame came from, for the manifest's `scene` and `frame`
/// (#1087): the scene file as given and the game time it was drawn at.
pub(super) struct Origin<'a> {
    pub scene: &'a str,
    pub game_time: Option<f64>,
}

impl Origin<'_> {
    /// Lay the origin, the writer and the moment on `description`.
    fn lay(&self, description: &mut flint_render::frame_capture::FrameDescription) {
        let bytes = std::fs::read(self.scene).ok();
        let name = bytes
            .as_deref()
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(|text| text.parse::<toml::Table>().ok())
            .and_then(|t| t.get("scene")?.get("name")?.as_str().map(String::from))
            .unwrap_or_default();
        description.scene = Some(flint_render::frame_capture::SceneDescription {
            path: self.scene.replace('\\', "/"),
            name,
            sha256: bytes.map(|b| flint_core::ContentHash::from_bytes(&b).to_hex()),
        });
        // As the player writes it (#1086): the version, the commit left
        // to a host that asks git
        description.engine = Some(flint_render::frame_capture::EngineDescription {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: None,
            dirty: None,
        });
        description.frame.game_time = self.game_time;
        description.written_by = Some("flint render".into());
        description.captured_at = Some(super::frame::utc_now());
    }
}

/// What [`arm`] changed, for [`write`] to put back.
pub(super) struct Armed {
    /// The timer was off before and is on for this frame.
    timing_turned_on: bool,
}

/// Describe the next frame `renderer` draws, timed when the adapter can,
/// and with `wanted.capture` copy each pass's targets out as it draws.
pub(super) fn arm(ctx: &HeadlessContext, renderer: &mut SceneRenderer, wanted: Wanted) -> Armed {
    if wanted.capture.is_some() {
        // The headless output and depth buffer are the host's: the
        // renderer draws into them without holding them
        renderer.capture_next_frame(flint_render::frame_capture::CaptureTargets {
            surface: Some(ctx.color_texture.clone()),
            depth: Some(ctx.depth_texture.clone()),
        });
    } else {
        renderer.describe_next_frame();
    }
    let was_on = renderer.gpu_timing();
    let on = renderer.set_gpu_timing(&ctx.device, &ctx.queue, true);
    if let Some(timer) = renderer.gpu_timer_mut() {
        timer.record(true);
    }
    Armed {
        timing_turned_on: on && !was_on,
    }
}

/// Write the frame drawn since [`arm`]: its description to `wanted.dump`,
/// its capture into `wanted.capture`, each with its GPU times laid on, and
/// say what was written, a line each. Call after the frame's pixels are
/// read back (the GPU is done).
pub(super) fn write(
    armed: Armed,
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    world: Option<&flint_ecs::FlintWorld>,
    wanted: Wanted,
    origin: Origin,
) -> Result<Vec<String>> {
    // The capture's copies, mapped and waited for: `flint render` has no
    // frame to keep (ADR 0126 decision 5)
    let captured = match renderer.take_frame_capture() {
        Some(mut capture) => {
            capture.wait(&ctx.device);
            Some(
                capture
                    .read()
                    .map_err(|e| anyhow::anyhow!("--frame-capture: {e}"))?,
            )
        }
        None => None,
    };
    // The frame's timestamps, resolved and read back now: nothing waits on
    // a headless frame
    renderer.finish_gpu_timings(&ctx.device, &ctx.queue);
    let timings = renderer.gpu_timer_mut().and_then(|timer| {
        timer.flush(&ctx.device);
        let last = timer.recorded().last().cloned();
        timer.record(false);
        last
    });
    if armed.timing_turned_on {
        renderer.set_gpu_timing(&ctx.device, &ctx.queue, false);
    }
    let host = flint_render::FrameHost {
        adapter: Some(&ctx.adapter_info),
        present_mode: None,
    };
    let mut description = renderer
        .describe_frame(world, &host)
        .context("the frame wasn't described (nothing was drawn)")?;
    if let Some(timings) = &timings {
        description.lay_gpu_times(timings);
    }
    origin.lay(&mut description);
    let timed = if timings.is_some() {
        ", GPU-timed"
    } else {
        " (untimed: the adapter has no TIMESTAMP_QUERY)"
    };
    let mut said = Vec::new();
    if let (Some(dir), Some(captured)) = (wanted.capture, captured) {
        let written = flint_render::frame_capture::write_capture(
            std::path::Path::new(dir),
            &mut description,
            captured,
        )
        .with_context(|| format!("--frame-capture: writing {dir}"))?;
        let passes = description
            .all_passes()
            .iter()
            .filter(|p| !p.images.is_empty())
            .count();
        let untapped = description.capture.as_ref().map_or(0, |c| c.untapped.len());
        said.push(format!(
            "Captured the frame to {dir}: {} images from {passes} passes, {:.1} MB on disk \
             ({:.1} MB read back){timed}{}",
            written.images,
            written.file_bytes as f64 / 1e6,
            written.raw_bytes as f64 / 1e6,
            if untapped > 0 {
                format!("; {untapped} written targets not read (manifest: capture.untapped)")
            } else {
                String::new()
            },
        ));
    }
    if let Some(path) = wanted.dump {
        let json = serde_json::to_string_pretty(&description).context("serialising the frame")?;
        if let Some(dir) = std::path::Path::new(path).parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir)
                    .with_context(|| format!("creating {}", dir.display()))?;
            }
        }
        std::fs::write(path, json).with_context(|| format!("writing {path}"))?;
        let all = description.all_passes();
        let ran = all.iter().filter(|p| p.ran).count();
        let draws: usize = description.pass("Scene").map_or(0, |p| p.draws.len());
        said.push(format!(
            "Wrote the frame's description to {path}: {ran} of {} passes ran, {draws} scene draws{timed}",
            all.len(),
        ));
    }
    Ok(said)
}
