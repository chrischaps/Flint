//! `flint render --frame-dump <file.json>` (#1084): the rendered frame's
//! description as JSON, the type a capture's `manifest.json` holds (ADR
//! 0126, `flint_render::frame_capture`).
//!
//! [`arm`] goes before the frame is drawn and [`write`] after it is read
//! back. While armed the renderer logs its passes; the GPU timer is on for
//! that one frame (where the adapter can time) and keeps its raw times, so
//! each pass carries what it cost in that frame, not a smoothed figure. A
//! render without the flag never arms it and draws as it always did.

use anyhow::{Context, Result};
use flint_render::{HeadlessContext, SceneRenderer};

/// What [`arm`] changed, for [`write`] to put back.
pub(super) struct Armed {
    /// The timer was off before and is on for this frame.
    timing_turned_on: bool,
}

/// Describe the next frame `renderer` draws, timed when the adapter can.
pub(super) fn arm(ctx: &HeadlessContext, renderer: &mut SceneRenderer) -> Armed {
    renderer.describe_next_frame();
    let was_on = renderer.gpu_timing();
    let on = renderer.set_gpu_timing(&ctx.device, &ctx.queue, true);
    if let Some(timer) = renderer.gpu_timer_mut() {
        timer.record(true);
    }
    Armed {
        timing_turned_on: on && !was_on,
    }
}

/// Write the frame drawn since [`arm`] to `path`, its GPU times laid on, and
/// say so. Call after the frame's pixels are read back (the GPU is done).
pub(super) fn write(
    armed: Armed,
    ctx: &HeadlessContext,
    renderer: &mut SceneRenderer,
    world: Option<&flint_ecs::FlintWorld>,
    path: &str,
) -> Result<String> {
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
        .context("--frame-dump: the frame wasn't described (nothing was drawn)")?;
    if let Some(timings) = &timings {
        description.lay_gpu_times(timings);
    }
    let json = serde_json::to_string_pretty(&description).context("serialising the frame")?;
    if let Some(dir) = std::path::Path::new(path).parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
    }
    std::fs::write(path, json).with_context(|| format!("writing {path}"))?;
    let all = description.all_passes();
    let ran = all.iter().filter(|p| p.ran).count();
    let draws: usize = description.pass("Scene").map_or(0, |p| p.draws.len());
    Ok(format!(
        "Wrote the frame's description to {path}: {ran} of {} passes ran, {draws} scene draws{}",
        all.len(),
        if timings.is_some() {
            ", GPU-timed"
        } else {
            " (untimed: the adapter has no TIMESTAMP_QUERY)"
        }
    ))
}
