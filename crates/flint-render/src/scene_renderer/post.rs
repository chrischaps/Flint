//! Post-processing, quality and style configuration (ADR 0118, #736): the
//! HDR chain's size and render scale, the drawn and authored post config
//! and its generation, quality tiers and MSAA, building styles' and FXAA's
//! resources, the style text/edge read-outs, and the post clock.

use super::*;

/// A number no other call in this process returns, for
/// [`SceneRenderer::post_config_generation`].
pub(super) fn next_post_config_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl SceneRenderer {
    /// Resize post-processing resources (call on window resize).
    /// `width` x `height` is the surface's size; the HDR chain is that at the
    /// render scale.
    pub fn resize_postprocess(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        let same_surface = self.surface_size == (width, height);
        self.surface_size = (width, height);
        let (w, h) = crate::render_scale::scaled_size(width, height, self.render_scale);
        // Stylize targets live in the resources, so they (and every bind
        // group over one) go with the old size (#262). Every bind group
        // does; but when only the render scale moved (`--render-scale
        // auto` stepping, #270) the surface is the same size, so the
        // display-stage targets are too, and they hold ASCII's per-cell
        // history (#277): keep them, and a step doesn't reset it. HDR-stage
        // targets in the set are remade at the new size on their next use.
        let kept = same_surface
            .then(|| self.postprocess_resources.take())
            .flatten()
            .map(|old| old.styles);
        let mut resources = PostProcessResources::new(device, w, h);
        if let Some(styles) = kept {
            resources.styles = styles;
        }
        self.postprocess_resources = Some(resources);
        // Below the surface's size the scene keeps a depth buffer of its own
        // (#122); at scale 1 it draws into the caller's, as it always has.
        if (w, h) == (width, height) {
            self.scaled_depth = None;
            self.depth_upscale = None;
        } else {
            let depth = crate::render_scale::ScaledDepth::new(device, w, h);
            self.depth_upscale = Some(crate::render_scale::DepthUpscale::new(device, &depth));
            self.scaled_depth = Some(std::sync::Arc::new(depth));
        }
        // The recreated resources drop the FXAA intermediate; re-allocate it
        // at the new size when the pass is active (ADR 0050).
        self.ensure_fxaa_resources(device);
    }

    /// Draw the scene at `scale` of the surface's size on each side, and
    /// upscale it in the composite pass (#122). What's drawn over the
    /// finished frame (world lines, world text, the HUD) stays at the
    /// surface's size. Clamped to [`MIN_RENDER_SCALE`]`..=1`; 1, the
    /// default, draws the scene exactly as before. Rebuilds the HDR chain
    /// when the size it gives changes.
    ///
    /// [`MIN_RENDER_SCALE`]: crate::MIN_RENDER_SCALE
    pub fn set_render_scale(&mut self, device: &wgpu::Device, scale: f32) {
        let scale = crate::render_scale::clamp_render_scale(scale);
        if scale == self.render_scale {
            return;
        }
        let (width, height) = self.surface_size;
        let before = crate::render_scale::scaled_size(width, height, self.render_scale);
        self.render_scale = scale;
        if crate::render_scale::scaled_size(width, height, scale) != before {
            self.resize_postprocess(device, width, height);
        }
    }

    /// Keep the `style` debug view's style (`mask_view`) on while that
    /// mode is set, whatever config was set since, and off once it isn't
    /// unless the config asked for it itself.
    pub(super) fn sync_debug_styles(&mut self) {
        let styles = &mut self.postprocess_config.styles;
        if self.debug_state.mode == DebugMode::StyleMask {
            if !styles.is_enabled("mask_view") {
                styles.set_enabled("mask_view", true);
                self.mask_view_forced = true;
            }
        } else if std::mem::take(&mut self.mask_view_forced) {
            styles.set_enabled("mask_view", false);
        }
        // The `edges` view (#349) is drawn by the ASCII style itself, so a
        // scene without it gets it, at its defaults, for the view
        if self.debug_state.mode == DebugMode::StyleEdges {
            if !styles.is_enabled("ascii") {
                styles.set_enabled("ascii", true);
                self.ascii_forced = true;
            }
        } else if std::mem::take(&mut self.ascii_forced) {
            styles.set_enabled("ascii", false);
        }
    }

    /// Bind groups the post chain keeps (its styles' among them). Steady
    /// from frame to frame while nothing new is switched on, so tests can
    /// show a per-frame change makes none (#269).
    pub fn kept_post_binds(&self) -> usize {
        self.postprocess_resources
            .as_ref()
            .map_or(0, |r| r.binds.len())
    }

    /// Bind groups the post chain has made since it was last rebuilt, the
    /// ones it made for a single frame included: a steady frame adds none
    /// once the caller's depth comes with an id (#379).
    pub fn post_binds_made(&self) -> u64 {
        self.postprocess_resources
            .as_ref()
            .map_or(0, |r| r.binds.made())
    }

    /// The render scale [`set_render_scale`](Self::set_render_scale) set.
    pub fn render_scale(&self) -> f32 {
        self.render_scale
    }

    /// The size the scene is drawn at: the HDR chain's, which is the
    /// surface's at scale 1.
    pub fn scene_size(&self) -> (u32, u32) {
        self.postprocess_resources
            .as_ref()
            .map_or(self.surface_size, |r| (r.width, r.height))
    }

    /// Ensure FXAA GPU resources exist. Every frame does this itself
    /// ([`encode_to`](Self::encode_to), #297), as it builds styles; calling
    /// it after setting a config moves the shader compile off the first
    /// frame that draws FXAA. No-op when `fxaa` is disabled — the
    /// default-off path allocates nothing (ADR 0050, Kuwahara lazy pattern).
    pub fn ensure_fxaa_resources(&mut self, device: &wgpu::Device) {
        if !self.postprocess_config.fxaa_enabled {
            return;
        }
        if let Some(pp) = &mut self.postprocess_pipeline {
            if pp.fxaa.is_none() {
                pp.fxaa = Some(crate::postprocess::FxaaPipeline::new(
                    device,
                    self.surface_format,
                ));
            }
        }
        if let Some(resources) = &mut self.postprocess_resources {
            if resources.fxaa.is_none() {
                let (w, h) = (resources.width, resources.height);
                resources.fxaa = Some(crate::postprocess::FxaaResources::new(
                    device,
                    w,
                    h,
                    self.surface_format,
                ));
            }
        }
    }

    /// This frame's drawing for the shading boil (#214), from the wooze field.
    pub fn set_boil(&mut self, boil: flint_wooze::BoilFrame) {
        self.postprocess_config.boil = boil;
        self.authored_postprocess.boil = boil;
    }

    /// The post-processing configuration as set: the scene's look, before
    /// the quality caps. Change a copy and hand it back with
    /// [`set_post_process_config`](Self::set_post_process_config).
    pub fn post_process_config(&self) -> &PostProcessConfig {
        &self.authored_postprocess
    }

    /// The configuration drawn: [`post_process_config`](Self::post_process_config)
    /// with the quality settings' caps applied.
    pub fn effective_post_process_config(&self) -> &PostProcessConfig {
        &self.postprocess_config
    }

    /// Set the post-processing configuration. It's drawn under the quality
    /// settings' caps (`set_quality`), which a scene load doesn't lift.
    pub fn set_post_process_config(&mut self, config: PostProcessConfig) {
        self.postprocess_config = crate::quality::mask_post_process(&config, &self.quality);
        self.authored_postprocess = config;
        self.post_config_generation = next_post_config_generation();
    }

    /// A number that changes whenever [`effective_post_process_config`]
    /// may have: a host keeping a copy of it re-reads it only then.
    ///
    /// [`effective_post_process_config`]: Self::effective_post_process_config
    pub fn post_config_generation(&self) -> u64 {
        self.post_config_generation
    }

    /// The quality settings in force; see [`set_quality`](Self::set_quality).
    pub fn quality(&self) -> &crate::quality::QualitySettings {
        &self.quality
    }

    /// Apply runtime quality settings (FlintKart#55) and return them as
    /// applied (clamped). Call between frames, never with a frame's passes
    /// half encoded: an MSAA change rebuilds every scene pipeline and its
    /// targets, a shadow-resolution change the shadow maps, and a render
    /// scale change the HDR chain. Only what changed is rebuilt, so calling
    /// it every frame with the same settings costs nothing.
    ///
    /// - `render_scale` sets the scene's scale as
    ///   [`set_render_scale`](Self::set_render_scale) does (a player stepping
    ///   the scale itself keeps calling that; `render_scale_auto` is the
    ///   player's and ignored here).
    /// - `msaa`: 1 or 4 samples for the scene passes.
    /// - `shadows`, `shadow_resolution`, `shadow_cascades`, `shadow_distance`.
    /// - `post_process`, `bloom`, `ssao`, `ssao_samples`, `fog`,
    ///   `volumetric` cap the post config: off turns the effect off, on
    ///   leaves it as set.
    pub fn set_quality(
        &mut self,
        device: &wgpu::Device,
        quality: &crate::quality::QualitySettings,
    ) -> crate::quality::QualitySettings {
        let q = quality.clamped();
        self.set_render_scale(device, q.render_scale);
        self.set_msaa(device, q.msaa);
        if q.shadow_resolution != self.shadow_resolution() {
            self.set_shadow_resolution(device, q.shadow_resolution);
        }
        self.set_shadows(q.shadows);
        if let Some(sp) = &mut self.shadow_pass {
            sp.set_cascades(q.shadow_cascades as usize);
            sp.set_distance(q.shadow_distance);
        }
        self.quality = q;
        self.postprocess_config = crate::quality::mask_post_process(&self.authored_postprocess, &q);
        self.post_config_generation = next_post_config_generation();
        q
    }

    /// The MSAA sample count the scene passes draw with.
    pub fn msaa(&self) -> u32 {
        self.pipelines.samples()
    }

    /// Draw the scene passes with `samples` MSAA samples, 1 or 4 (anything
    /// else is 1, with a warning). Rebuilds every scene pipeline, whose
    /// sample count is baked in, and drops the MSAA targets so the next
    /// frame makes them at the new count. Bind groups, meshes and textures
    /// are kept: the new pipelines are made against layouts equal to the
    /// old ones, which wgpu (and WebGPU) treat as the same layout.
    pub fn set_msaa(&mut self, device: &wgpu::Device, samples: u32) -> u32 {
        let samples = Self::resolve_sample_count(samples);
        if samples == self.pipelines.samples() {
            return samples;
        }
        self.quality.msaa = samples;
        // The whole set at once, so no pipeline can be left at the old count.
        self.pipelines = self.pipelines.rebuilt(device, samples, &self.layouts);

        // The targets are remade at the new count by the next frame.
        self.msaa.drop_targets();
        tracing::info!("[quality] MSAA {samples}x");
        samples
    }

    /// Build the pipelines of every stylize pass the config switches on
    /// that isn't built yet (#262). Every frame does this itself; calling
    /// it after setting a config moves the shader compile off the first
    /// frame that draws the style. A style's textures are made on that
    /// frame.
    ///
    /// Building runs under `catch_unwind` because some GPU drivers (Intel
    /// Skylake) crash with unrecoverable device loss compiling these
    /// shaders; a style that fails stays off and marks the device lost.
    pub fn ensure_style_resources(&mut self, device: &wgpu::Device) {
        let Some(pp) = &mut self.postprocess_pipeline else {
            return;
        };
        let surface_format = pp.surface_format;
        let mut failures = Vec::new();
        for failed in pp.styles.ensure(
            device,
            surface_format,
            &self.postprocess_config,
            &self.style_mask.ids,
        ) {
            failures.push(failed);
        }
        // A style built again from a changed shader file (#418) made new
        // buffers: the kept bind groups hold the old ones
        if pp.styles.take_rebuilt() {
            if let Some(resources) = &self.postprocess_resources {
                resources.binds.clear();
            }
        }
        // A user style replaced by an edit (#275) left its old groups under
        // names nothing asks for again (#427). Asked of whichever view is
        // swapped in, so each comic panel's cache drops them at its draw
        if let Some(resources) = &self.postprocess_resources {
            resources.binds.evict_retired();
        }
        for failed in failures {
            if !crate::stylize::is_native(failed.name) {
                // A user style (ADR 0112) that panicked building is its
                // author's bug, not a lost device: it stays off and says so
                crate::stylize::custom::report_build_panic(failed.name);
                self.postprocess_config
                    .styles
                    .set_enabled(failed.name, false);
                continue;
            }
            if crate::stylize::is_game_style(failed.name) {
                // A game's Rust style (#419) whose `create` panicked is the
                // game's bug too: off, said once, the device kept
                tracing::error!(
                    "game style '{}' panicked building its pipelines; it stays off",
                    failed.name
                );
                self.postprocess_config
                    .styles
                    .set_enabled(failed.name, false);
                self.authored_postprocess
                    .styles
                    .set_enabled(failed.name, false);
                continue;
            }
            tracing::warn!(
                "{} style unavailable: GPU driver crashed during shader compilation. \
                 Try updating your graphics drivers.",
                failed.label
            );
            self.postprocess_config
                .styles
                .set_enabled(failed.name, false);
            self.authored_postprocess
                .styles
                .set_enabled(failed.name, false);
            self.device_lost = true;
        }
    }

    /// How many times the shared style edge pass (#301) has been drawn
    /// since this renderer was made: one a frame while any style that
    /// needs the edges is on, however many do.
    /// Have glyph styles (ASCII) keep each frame's character grid as text
    /// (#317), for [`style_text`](Self::style_text). Costs one pass of a
    /// fragment per cell and a copy to the CPU a frame, so it's off unless
    /// asked for. Does nothing without post-processing, where no style runs.
    pub fn set_style_text_capture(&mut self, on: bool) {
        if let Some(pp) = &mut self.postprocess_pipeline {
            pp.styles.set_text_capture(on);
        }
    }

    /// The character grid style `name` drew in the last frame rendered
    /// with text capture on, once its submission is done (this waits on
    /// the device). `None` if that style draws no glyphs or didn't run.
    pub fn style_text(
        &self,
        device: &wgpu::Device,
        name: &str,
    ) -> Option<crate::stylize::GlyphText> {
        self.postprocess_pipeline
            .as_ref()?
            .styles
            .read_text(name, device)
    }

    pub fn style_edge_runs(&self) -> u64 {
        self.postprocess_pipeline
            .as_ref()
            .map_or(0, |pp| pp.styles.edge_runs())
    }

    /// How many times the framework's cross-fade (#396) has been drawn:
    /// once after each style it fades, on each frame some mask pixel is
    /// below a blend of 1.
    pub fn style_fade_runs(&self) -> u64 {
        self.postprocess_pipeline
            .as_ref()
            .map_or(0, |pp| pp.styles.fade_runs())
    }

    /// The old name of [`ensure_style_resources`](Self::ensure_style_resources),
    /// from when Kuwahara was the only style.
    pub fn ensure_kuwahara_resources(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        self.ensure_style_resources(device);
    }

    /// Validate the configured MSAA sample count (ADR 0058): 1 or 4;
    /// anything else clamps to 1 with a warning.
    pub(super) fn resolve_sample_count(requested: u32) -> u32 {
        match requested {
            1 | 4 => requested,
            other => {
                tracing::warn!(
                    "Unsupported MSAA sample count {} - falling back to 1 (supported: 1, 4)",
                    other
                );
                1
            }
        }
    }

    /// Forget what styles remember of earlier frames (ASCII's temporal
    /// stability, #277): call it when the picture stops following on from
    /// the last one, on a scene change or a camera cut. The next frame is
    /// drawn as if it were the first.
    pub fn reset_style_history(&mut self) {
        self.style_history_epoch = self.style_history_epoch.wrapping_add(1);
    }

    /// The post time styles, grain and the tear read this frame: the
    /// `ocean_time` clock wrapped as [`motion_hold::post_time`]
    /// (crate::motion_hold::post_time) does, held to the start of its tick
    /// under a whole-frame `[post_process] motion_hold` (#405).
    pub fn post_time(&self) -> f32 {
        let time = crate::motion_hold::post_time(self.ocean_time);
        crate::motion_hold::held_time(time, self.postprocess_config.motion_hold)
    }

    /// The style chain's textures (#302): see
    /// [`StyleTargets::summary`](crate::stylize::StyleTargets::summary).
    pub fn style_target_summary(&self) -> crate::stylize::TargetSummary {
        self.postprocess_resources
            .as_ref()
            .map(|r| {
                r.styles.summary(
                    self.surface_format,
                    self.surface_size.0,
                    self.surface_size.1,
                )
            })
            .unwrap_or_default()
    }

    /// Each of the style chain's textures with its size and bytes (#412):
    /// the built-ins' and every user style's targets (`<style>.<target>`,
    /// `.a`/`.b` for a history pair), sorted by name.
    pub fn style_target_entries(&self) -> Vec<crate::stylize::TargetEntry> {
        self.postprocess_resources
            .as_ref()
            .map(|r| r.styles.entries())
            .unwrap_or_default()
    }
}
